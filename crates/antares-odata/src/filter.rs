// SPDX-License-Identifier: EUPL-1.2
//! `$filter` to NGSI-LD `q` and `id` (odata-linkml-mapping.md 5.2), and
//! `$orderby` to `orderBy` (Q9: `id` and scalar Attributes). The
//! subset of this round: comparisons of a scalar member with a literal
//! (F1 to F6, F26), `and`/`or` (F7, F8), `null` (F2, F4, F10), `in` (F12),
//! `contains`/`startswith`/`endswith` and their `not` (F9, F15), `id`
//! as a top-level conjunct (F28), `observedAt` (F21), membership in a
//! ListProperty (F22), `geo.intersects` as a top-level conjunct (F29), and
//! `eq` on a LanguageProperty (F31) or a VocabProperty (F32).
//!
//! The filter is split at its top-level `and`s (ADR-0028 rule 1, T-3758): a
//! conjunct `q`, `id` or the one `geoQ` can say is pushed; any other the
//! residual evaluator covers goes, whole, to [`Translated::residual`]: `not`
//! over the rows above (F11), a second `geo.intersects` or `id` conjunct, a
//! date-time literal with an offset. OData compares a missing value as false
//! (URL 5.1.1.1.2, 5.1.1.1.3), so a residual comparison is made definite
//! ([`definite`]) and `not` of it holds where the member is absent. Everything
//! else is `Later` (501) or `Bad` (400) before any operation runs.
//!
//! A `q` names Attributes by term, and a term is a letter followed by
//! letters, digits or `_` (7.2.3.2), and `orderBy` reads a `.` as a path
//! (4.23): a schema IRI is neither. Each IRI the request names gets the
//! term `t{n}`, and the request carries the
//! `@context` that defines them (`OpsRequest::context`).

use crate::edm::{Member, Set, Shape};
use crate::functions::{signature, NUMBER};
use antares_ops::residual::{
    self, decode_base64, Cmp, Date, DateTime, Duration, Expr as RExpr, Func, Lit,
    Member as RMember, Path, Spatial, Time,
};
use odata_url::{
    ArithOp, CompareOp, Compute, Expr, Geography, Literal, MemberPath, Method, OrderBy, Search,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The broker's bound on a `q` (`antares_ql::MAX_Q_BYTES`): a longer
/// translation is refused here with the OData error a bound gets.
pub const MAX_Q_BYTES: usize = 4096;

/// The instance annotation of a sub-Property `observedAt` (S1, F21).
const OBSERVED_AT: &str = "@NgsiLd.observedAt";

/// The largest integer an `f64` holds exactly: NGSI-LD compares numbers as
/// `f64` (F26).
const MAX_EXACT: i64 = 1 << 53;

/// Why a filter is not translated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    /// A construct a later round maps or no round does: 501.
    Later,
    /// A filter that cannot hold for this model: 400.
    Bad,
    /// Past the broker's bound on a `q`: 400 `TooComplexQuery`.
    TooComplex,
}

/// A translated filter.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Translated {
    /// The `q`, if any term needs one.
    pub(crate) q: Option<String>,
    /// The ids of an `id` conjunct.
    pub(crate) ids: Option<Vec<String>>,
    /// The `orderBy`, if the request orders.
    pub(crate) order: Option<String>,
    /// The terms `q` and `orderBy` use, term to IRI.
    pub(crate) context: Map<String, Value>,
    /// How many Relationships deep `q` follows one (F24, T-5906), 0 for none: the broker
    /// evaluates a hop only under a `join` at least that deep (5.7.2.4).
    pub(crate) linked: usize,
    /// F29: the `geoQ` of a `geo.intersects` conjunct, the GeoProperty's
    /// term and the polygon's `coordinates`.
    pub(crate) geo: Option<(String, String)>,
    /// F32: the VocabProperty terms whose compared values the broker
    /// expands (`expandValues`, 7.2.3.2 EXAMPLE 13).
    pub(crate) expand_values: Vec<String>,
    /// The conjuncts the store cannot say, each true for an Entity in the
    /// answer (ADR-0028 rule 1); empty when the filter pushes whole.
    pub(crate) residual: Vec<RExpr>,
    /// The pushed `geo` conjunct as a residual one, for a request whose
    /// `ngsild.georel` takes the one `geoQ` (ADR-0028 rule 6).
    pub(crate) geo_residual: Option<RExpr>,
    /// The `$orderby` when a key is one `orderBy` cannot name (Q9): the
    /// surface sorts the whole answer by these keys, `order` is then `None`.
    pub(crate) sort: Option<Vec<SortKey>>,
    /// The Attribute terms the surface sort reads, for the pushdown's `attrs`.
    pub(crate) sort_reads: Vec<String>,
    /// Whether the surface sort reads `createdAt` or `modifiedAt`.
    pub(crate) sort_system: bool,
    /// The keys of a pushed `orderBy`, for a read that finds an Entity
    /// lacking one and sorts on the surface instead (T-3853).
    pub(crate) pushed_keys: Vec<SortKey>,
    /// The terms of the pushed value keys the `$filter` holds on every
    /// Entity it admits ([`holds_member`]): no Entity of the answer lacks
    /// one, so no read counts those that do (T-4919).
    pub(crate) held_keys: Vec<String>,
    /// The `ngsild.*` parameters pushed beside the rest (`&scopeQ=…`,
    /// `&georel=…`), encoded (ADR-0028 rule 6).
    pub(crate) ngsild: String,
    /// `Prefer: odata.include-annotations` names the surface's vocabulary: each member
    /// carries its `observedAt`, `unitCode` and `datasetId` as instance annotations (T-5206).
    pub(crate) annotate: bool,
    /// The `$compute` items as residual values, name and value, in the
    /// request's order (Q16).
    pub(crate) computed: Vec<(String, RExpr)>,
    /// The Edm type of each computed value, in the order of `computed`.
    pub(crate) computed_types: Vec<&'static str>,
    /// The Attribute terms the computed values read, for the pushdown's
    /// `attrs`.
    pub(crate) computed_reads: Vec<String>,
}

impl Translated {
    /// What selects the Entities, without the order, the residual or the
    /// surface sort: the base of a count that asks about the same answer.
    pub(crate) fn probe(&self) -> Self {
        Self {
            q: self.q.clone(),
            ids: self.ids.clone(),
            context: self.context.clone(),
            linked: self.linked,
            geo: self.geo.clone(),
            expand_values: self.expand_values.clone(),
            ngsild: self.ngsild.clone(),
            ..Self::default()
        }
    }
}

/// One key of a surface sort (Q9, ADR-0028 rule 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SortKey {
    /// What the key reads.
    pub(crate) by: SortBy,
    /// `desc`.
    pub(crate) descending: bool,
}

/// The value a [`SortKey`] reads from an Entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SortBy {
    /// The Entity id.
    Id,
    /// The value of the Attribute of this term, at its default instance.
    Value(String),
    /// `createdAt` or `modifiedAt` of the Entity.
    System(String),
    /// The `$compute` value of this name.
    Computed(String),
    /// The value of the Attribute of the second term on the Entity the Relationship of the
    /// first term targets, as the join inlined it (T-5906).
    Linked(String, String),
}

impl SortBy {
    /// The `orderBy` key of a pushed sort.
    fn key(&self) -> &str {
        match self {
            Self::Id => "id",
            Self::Value(name)
            | Self::System(name)
            | Self::Computed(name)
            | Self::Linked(name, _) => name,
        }
    }
}

/// Translates `filter` against the members of `set` and, through a
/// navigation, of the `sets` it targets, with the parameter aliases of the
/// request bound. A LanguageProperty is compared in `lang` (`ngsild.lang`, else its `@none`
/// entry), the text the read serves (T-5206).
pub(crate) fn translate(
    filter: &Expr,
    set: &Set,
    sets: &BTreeMap<String, Set>,
    aliases: &[(String, Literal)],
    lang: Option<&str>,
) -> Result<Translated, Refused> {
    let mut out = Translated::default();
    let mut terms = Terms {
        set,
        sets,
        aliases,
        context: &mut out.context,
        linked: 0,
        expand_values: Vec::new(),
        lambdas: Vec::new(),
        hops: Vec::new(),
        lang: lang.unwrap_or("@none"),
    };
    let mut conjuncts = Vec::new();
    for conjunct in conjuncts_of(filter) {
        if let Some((term, polygon)) = terms.geo(conjunct)? {
            // one geoQ per query (4.10): a second area is the residual's
            let geo = intersects(&term, &polygon);
            if out.geo.is_some() {
                out.residual.push(geo);
            } else {
                out.geo = Some((term, polygon.to_string()));
                out.geo_residual = Some(geo);
            }
            continue;
        }
        match ids_of(conjunct, aliases)? {
            Some(ids) if out.ids.is_none() => out.ids = Some(ids),
            // one id parameter per query: a second list is the residual's
            Some(ids) => out.residual.push(RExpr::In(
                Box::new(RExpr::Path(Path::Id)),
                ids.into_iter().map(Lit::Str).collect(),
            )),
            None => {
                let mark = (terms.linked, terms.expand_values.len());
                match terms.term(conjunct) {
                    Ok(q) => conjuncts.push(format!("({q})")),
                    Err(Refused::Later) => {
                        // what the failed translation marked is not pushed
                        terms.linked = mark.0;
                        terms.expand_values.truncate(mark.1);
                        out.residual.push(terms.residual(conjunct)?);
                    }
                    Err(refused) => return Err(refused),
                }
            }
        }
    }
    out.linked = terms.linked;
    out.expand_values = terms.expand_values;
    if !conjuncts.is_empty() {
        let q = conjuncts.join(";");
        if q.len() > MAX_Q_BYTES {
            return Err(Refused::TooComplex);
        }
        out.q = Some(q);
    }
    Ok(out)
}

/// `filter` over a history set (T-5908): the first `id` conjunct as `ids`, every other conjunct
/// residual, which the surface evaluates over the rows it builds from the temporal answer.
pub(crate) fn residual_whole(
    filter: &Expr,
    set: &Set,
    aliases: &[(String, Literal)],
) -> Result<Translated, Refused> {
    let mut out = Translated::default();
    let none = BTreeMap::new();
    let mut terms = Terms {
        set,
        sets: &none,
        aliases,
        context: &mut out.context,
        linked: 0,
        expand_values: Vec::new(),
        lambdas: Vec::new(),
        hops: Vec::new(),
        lang: "@none",
    };
    let (mut ids, mut residual) = (None, Vec::new());
    for conjunct in conjuncts_of(filter) {
        match ids_of(conjunct, aliases)? {
            Some(found) if ids.is_none() => ids = Some(found),
            Some(found) => residual.push(RExpr::In(
                Box::new(RExpr::Path(Path::Id)),
                found.into_iter().map(Lit::Str).collect(),
            )),
            None => residual.push(terms.residual(conjunct)?),
        }
    }
    out.ids = ids;
    out.residual = residual;
    Ok(out)
}

/// The instants the top-level conjuncts bound `member` by (T-5908): each `ge`, `gt` or `eq`
/// literal a lower bound, each `lt` or `le` literal an upper one, `le` and `eq` with whether
/// the bound is inclusive. Any other conjunct bounds nothing.
pub(crate) fn bounds_of<'e>(
    filter: &'e Expr,
    member: &str,
    aliases: &'e [(String, Literal)],
) -> Vec<(CompareOp, &'e str)> {
    conjuncts_of(filter)
        .into_iter()
        .filter_map(|c| {
            let Expr::Compare(l, op, r) = c else {
                return None;
            };
            let named = |e: &Expr| matches!(plain(e), Some([only]) if only == member);
            let (op, lit) = if named(l) {
                (*op, literal(r, aliases).ok()?)
            } else if named(r) {
                (mirrored(*op), literal(l, aliases).ok()?)
            } else {
                return None;
            };
            match lit {
                Literal::DateTimeOffset(t) => Some((op, t.as_str())),
                _ => None,
            }
        })
        .collect()
}

/// `$search` (URL 5.1.7, Q14) as the `$filter` conjunct it means here: a word or phrase
/// matches an Entity when one of its single-valued `Edm.String` properties the view shows holds
/// it, case-insensitively (`tolower` on both sides); `NOT`, `AND` and `OR` as in a filter. An
/// absent property holds nothing, so `NOT` of a term holds where every property is absent. A
/// property the view or the engine hides is not in the answer the residual reads, so a search
/// never matches through it. `None` when the search holds for every Entity.
///
/// Decided (worker-5, owner may override): substring matching over the String properties, no
/// stemming or ranking, because the residual evaluator holds `contains` and `tolower` and an
/// Entity either matches or not (5.1.7 leaves the matching rule to the service); options: whole
/// words only, or the Relationships' object URIs too.
pub(crate) fn searched(search: &Search, set: &Set) -> Option<Expr> {
    let members: Vec<&str> = set
        .members
        .iter()
        .filter(|(_, m)| {
            matches!(
                m.shape,
                Shape::Value {
                    edm: "Edm.String",
                    collection: false
                }
            )
        })
        .map(|(name, _)| name.as_str())
        .collect();
    if members.is_empty() {
        // no property can hold a term: the search is a constant
        return (!holds_nothing(search)).then_some(Expr::Literal(Literal::Bool(false)));
    }
    Some(matched(search, &members))
}

/// Whether `search` holds for an Entity whose properties hold no term.
fn holds_nothing(search: &Search) -> bool {
    match search {
        Search::Word(_) | Search::Phrase(_) => false,
        Search::Not(operand) => !holds_nothing(operand),
        Search::And(left, right) => holds_nothing(left) && holds_nothing(right),
        Search::Or(left, right) => holds_nothing(left) || holds_nothing(right),
    }
}

/// `search` as a filter over `members`, at least one.
fn matched(search: &Search, members: &[&str]) -> Expr {
    let both =
        |l: &Search, r: &Search| (Box::new(matched(l, members)), Box::new(matched(r, members)));
    match search {
        Search::Word(text) | Search::Phrase(text) => {
            let needle = Literal::String(text.to_lowercase());
            let one = |name: &str| {
                let member = || {
                    Expr::Member(MemberPath {
                        variable: None,
                        path: vec![name.to_owned()],
                    })
                };
                let present = Expr::Compare(
                    Box::new(member()),
                    CompareOp::Ne,
                    Box::new(Expr::Literal(Literal::Null)),
                );
                let holds = Expr::Call(
                    Method::Contains,
                    vec![
                        Expr::Call(Method::ToLower, vec![member()]),
                        Expr::Literal(needle.clone()),
                    ],
                );
                Expr::And(Box::new(present), Box::new(holds))
            };
            let mut terms = members.iter().map(|name| one(name));
            let first = terms.next().unwrap_or(Expr::Literal(Literal::Bool(false)));
            terms.fold(first, |a, b| Expr::Or(Box::new(a), Box::new(b)))
        }
        Search::Not(operand) => Expr::Not(Box::new(matched(operand, members))),
        Search::And(left, right) => {
            let (l, r) = both(left, right);
            Expr::And(l, r)
        }
        Search::Or(left, right) => {
            let (l, r) = both(left, right);
            Expr::Or(l, r)
        }
    }
}

/// Whether `filter` holds only on an Entity that has the member `name`: a
/// conjunct, or each side of a disjunction, compares it with `eq`, `gt`,
/// `ge`, `lt` or `le` to a literal other than `null`, and none of those
/// holds on a null (URL 5.1.1.1; `ne` does).
pub(crate) fn holds_member(filter: &Expr, name: &str) -> bool {
    let named = |e: &Expr| matches!(e, Expr::Member(MemberPath { variable: None, path }) if path.as_slice() == [name]);
    let literal = |e: &Expr| matches!(e, Expr::Literal(l) if *l != Literal::Null);
    match filter {
        Expr::And(l, r) => holds_member(l, name) || holds_member(r, name),
        Expr::Or(l, r) => holds_member(l, name) && holds_member(r, name),
        Expr::Compare(l, op, r) => {
            !matches!(op, CompareOp::Ne) && (named(l) && literal(r) || literal(l) && named(r))
        }
        _ => false,
    }
}

/// Adds `$orderby` to `out` as `orderBy` (Q9) over the same terms: `id`, or
/// a scalar Attribute; a member the model marks not sortable (C12) is 400,
/// the base members and the other shapes are a later round's. A scalar of the
/// target of a single-valued navigation (`Category/Name`, URL 5.1.5) has no
/// `orderBy` form: the surface sorts by it over the joined answer (T-5906).
pub(crate) fn order(
    items: &[OrderBy],
    set: &Set,
    sets: &BTreeMap<String, Set>,
    out: &mut Translated,
) -> Result<(), Refused> {
    let mut terms = Terms {
        set,
        sets,
        aliases: &[],
        context: &mut out.context,
        linked: 0,
        expand_values: Vec::new(),
        lambdas: Vec::new(),
        hops: Vec::new(),
        lang: "@none",
    };
    let mut keys = Vec::new();
    for item in items {
        let (single, hop) = match &item.expr {
            Expr::Member(MemberPath {
                variable: None,
                path,
            }) => match path.as_slice() {
                [name] => (Some(name.as_str()), None),
                [nav, name] => (None, Some((nav, name))),
                _ => (None, None),
            },
            _ => (None, None),
        };
        if let Some(hop) = hop {
            keys.push(SortKey {
                by: terms.linked_key(hop)?,
                descending: item.descending,
            });
            continue;
        }
        let by = match single {
            None => return Err(Refused::Later),
            Some(name) if out.computed.iter().any(|(c, _)| c == name) => {
                SortBy::Computed(name.to_owned())
            }
            Some("id") => SortBy::Id,
            // Q9: no Entity member `orderBy` names (7.2.3.1); the surface sorts
            Some(name @ ("createdAt" | "modifiedAt")) if !set.members.contains_key(name) => {
                SortBy::System(name.to_owned())
            }
            Some(name) if set.members.get(name).is_some_and(|m| !m.shape.sortable()) => {
                return Err(Refused::Bad)
            }
            _ => SortBy::Value(terms.scalar(&item.expr)?),
        };
        keys.push(SortKey {
            by,
            descending: item.descending,
        });
    }
    if keys.iter().any(|k| {
        matches!(
            k.by,
            SortBy::System(_) | SortBy::Computed(_) | SortBy::Linked(..)
        )
    }) {
        out.linked = out.linked.max(terms.linked);
        out.sort_system = true;
        out.sort_reads = keys
            .iter()
            .filter_map(|k| match &k.by {
                SortBy::Value(term) | SortBy::Linked(term, _) => Some(term.clone()),
                SortBy::Id | SortBy::System(_) | SortBy::Computed(_) => None,
            })
            .collect();
        out.sort = Some(keys);
        return Ok(());
    }
    let pushed: Vec<String> = keys
        .iter()
        .map(|k| {
            let key = k.by.key().to_owned();
            if k.descending {
                format!("{key};desc")
            } else {
                key
            }
        })
        .collect();
    out.order = Some(pushed.join(","));
    out.pushed_keys = keys;
    Ok(())
}

/// `$compute` (URL 5.1.10, Q16): each item as a residual value over the
/// members of `set` into `out.computed`, with the terms the values read
/// (a system member is no residual member here: `createdAt` is the model's
/// or 501). The scalar members, functions and arithmetic a `$filter`
/// residual takes; a navigation is a later round's.
pub(crate) fn compute(
    items: &[Compute],
    set: &Set,
    aliases: &[(String, Literal)],
    out: &mut Translated,
    now: DateTime,
) -> Result<(), Refused> {
    let sets = BTreeMap::new();
    let mut terms = Terms {
        set,
        sets: &sets,
        aliases,
        context: &mut out.context,
        linked: 0,
        expand_values: Vec::new(),
        lambdas: Vec::new(),
        hops: Vec::new(),
        lang: "@none",
    };
    for item in items {
        let (value, edm) = terms.residual_value(&item.expr)?;
        out.computed.push((item.alias.clone(), value));
        out.computed_types.push(edm);
    }
    for (_, program) in computed(out, now)? {
        for term in program.attributes() {
            if !out.computed_reads.iter().any(|t| t == term) {
                out.computed_reads.push(term.to_owned());
            }
        }
    }
    Ok(())
}

/// The computed values of `t` as programs, by name.
///
/// # Errors
/// As [`program`].
pub(crate) fn computed(
    t: &Translated,
    now: DateTime,
) -> Result<Vec<(String, residual::Program)>, Refused> {
    t.computed
        .iter()
        .map(|(name, value)| Ok((name.clone(), compiled(value, now)?)))
        .collect()
}

/// `e` with each `$compute` name replaced by its expression: a filter on
/// a computed value is a filter on what it computes.
pub(crate) fn inlined(e: &Expr, items: &[Compute]) -> Expr {
    let boxed = |e: &Expr| Box::new(inlined(e, items));
    match e {
        Expr::Member(MemberPath {
            variable: None,
            path,
        }) => match path.as_slice() {
            [name] => items
                .iter()
                .find(|c| c.alias == *name)
                .map_or_else(|| e.clone(), |c| c.expr.clone()),
            _ => e.clone(),
        },
        Expr::Compare(l, op, r) => Expr::Compare(boxed(l), *op, boxed(r)),
        Expr::In(l, list) => Expr::In(boxed(l), list.clone()),
        Expr::And(l, r) => Expr::And(boxed(l), boxed(r)),
        Expr::Or(l, r) => Expr::Or(boxed(l), boxed(r)),
        Expr::Not(inner) => Expr::Not(boxed(inner)),
        Expr::Call(method, args) => {
            Expr::Call(*method, args.iter().map(|a| inlined(a, items)).collect())
        }
        Expr::Arith(l, op, r) => Expr::Arith(boxed(l), *op, boxed(r)),
        Expr::Neg(inner) => Expr::Neg(boxed(inner)),
        Expr::Any { collection, lambda } => Expr::Any {
            collection: collection.clone(),
            lambda: lambda
                .as_ref()
                .map(|(variable, body)| (variable.clone(), boxed(body))),
        },
        Expr::All {
            collection,
            lambda: (variable, body),
        } => Expr::All {
            collection: collection.clone(),
            lambda: (variable.clone(), boxed(body)),
        },
        Expr::Literal(_) | Expr::Alias(_) | Expr::Member(_) => e.clone(),
    }
}

/// The operands of the top-level `and`s.
fn conjuncts_of(e: &Expr) -> Vec<&Expr> {
    match e {
        Expr::And(l, r) => {
            let mut all = conjuncts_of(l);
            all.extend(conjuncts_of(r));
            all
        }
        other => vec![other],
    }
}

/// F28: `id eq 'u'` or `id in ('u', ...)`, `None` for any other conjunct.
fn ids_of(e: &Expr, aliases: &[(String, Literal)]) -> Result<Option<Vec<String>>, Refused> {
    let is_id = |m: &Expr| matches!(m, Expr::Member(MemberPath { variable: None, path }) if path.len() == 1 && path.first().is_some_and(|p| p == "id"));
    let literals: Vec<&Literal> = match e {
        Expr::Compare(l, CompareOp::Eq, r) if is_id(l) => vec![literal(r, aliases)?],
        Expr::Compare(l, CompareOp::Eq, r) if is_id(r) => vec![literal(l, aliases)?],
        Expr::In(l, list) if is_id(l) => list.iter().collect(),
        _ => return Ok(None),
    };
    literals
        .into_iter()
        .map(|l| match l {
            Literal::String(s) => Ok(s.clone()),
            _ => Err(Refused::Bad),
        })
        .collect::<Result<_, _>>()
        .map(Some)
}

/// The literal an operand is, through an alias.
fn literal<'a>(e: &'a Expr, aliases: &'a [(String, Literal)]) -> Result<&'a Literal, Refused> {
    match e {
        Expr::Literal(l) => Ok(l),
        Expr::Alias(name) => aliases
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, l)| l)
            .ok_or(Refused::Bad),
        _ => Err(Refused::Later),
    }
}

struct Terms<'a> {
    set: &'a Set,
    sets: &'a BTreeMap<String, Set>,
    aliases: &'a [(String, Literal)],
    context: &'a mut Map<String, Value>,
    linked: usize,
    expand_values: Vec<String>,
    /// The `any`/`all` variables the residual is inside, innermost last,
    /// each with the Edm type of its list's items.
    lambdas: Vec<(String, &'static str)>,
    /// The `any` variables over a collection-valued navigation `q` is inside, innermost last.
    hops: Vec<Hop<'a>>,
    /// The `LanguageMap` entry a LanguageProperty is compared in, as it is read.
    lang: &'a str,
}

/// An `any` variable over a collection-valued navigation (T-5906): an item is an Entity of
/// `set`, and a member `m` of it is the linked-entity term `{open}m` closed by `depth` braces
/// (TS 104 175 4.9, `rel{Type:m}`).
struct Hop<'a> {
    variable: String,
    set: &'a Set,
    open: String,
    depth: usize,
}

impl<'a> Terms<'a> {
    /// One boolean expression as `q`.
    fn term(&mut self, e: &Expr) -> Result<String, Refused> {
        match e {
            // Inside `any` over a navigation each term may match another target: `and` is the
            // residual's, `or` distributes (`any(a or b)` is `any(a) or any(b)`).
            Expr::And(..) if !self.hops.is_empty() => Err(Refused::Later),
            Expr::And(l, r) => Ok(format!("({});({})", self.term(l)?, self.term(r)?)),
            Expr::Or(l, r) => Ok(format!("({})|({})", self.term(l)?, self.term(r)?)),
            Expr::Compare(l, op, r) => self.compare(l, *op, r),
            Expr::In(l, list) => {
                let (name, edm) = self.typed_member(l)?;
                typed(edm, list)?;
                let values = list.iter().map(value).collect::<Result<Vec<_>, _>>()?;
                Ok(format!("{name}=={}", values.join(",")))
            }
            Expr::Call(method, args) => self.call(*method, args, false),
            Expr::Not(inner) => match inner.as_ref() {
                Expr::Call(method, args) => self.call(*method, args, true),
                Expr::Compare(l, CompareOp::Eq, r)
                    if matches!(literal(r, self.aliases), Ok(Literal::Null)) =>
                {
                    Ok(self.scalar(l)?)
                }
                // F11: `q` has no general negation.
                _ => Err(Refused::Later),
            },
            // `$filter=flag`: a Boolean member as the predicate.
            Expr::Member(_) => Ok(format!("{}==true", self.scalar(e)?)),
            Expr::Any { collection, lambda } if self.navigation(collection).is_some() => {
                self.linked_any(collection, lambda.as_ref())
            }
            Expr::Any {
                collection,
                lambda: Some((variable, body)),
            } => self.any(collection, variable, body),
            // Arithmetic and `all` have no `q` form (F14, F23).
            Expr::Literal(_)
            | Expr::Alias(_)
            | Expr::Any { .. }
            | Expr::All { .. }
            | Expr::Arith(..)
            | Expr::Neg(_) => Err(Refused::Later),
        }
    }

    /// F22: `p/any(x: x eq v)` or `p/any(x: x in (...))` over a
    /// ListProperty is membership, which is what `==` on a list is in `q`
    /// (7.2.3.4); any other body is F23.
    fn any(
        &mut self,
        collection: &MemberPath,
        variable: &str,
        body: &Expr,
    ) -> Result<String, Refused> {
        let item = |e: &Expr| matches!(e, Expr::Member(MemberPath { variable: Some(v), path }) if v == variable && path.is_empty());
        let values: Vec<&Literal> = match body {
            Expr::Compare(l, CompareOp::Eq, r) if item(l) => vec![literal(r, self.aliases)?],
            Expr::Compare(l, CompareOp::Eq, r) if item(r) => vec![literal(l, self.aliases)?],
            Expr::In(l, list) if item(l) => list.iter().collect(),
            _ => return Err(Refused::Later),
        };
        let member = match collection {
            MemberPath {
                variable: None,
                path,
            } => match self.leaf(path) {
                Some((None, m)) => m,
                _ => return Err(Refused::Later),
            },
            MemberPath { .. } => return Err(Refused::Later),
        };
        if !matches!(
            member.shape,
            Shape::Value {
                collection: true,
                ..
            }
        ) {
            return Err(Refused::Later);
        }
        let values = values
            .into_iter()
            .map(value)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(format!(
            "{}=={}",
            self.term_of(&member.iri),
            values.join(",")
        ))
    }

    /// The collection-valued navigation a lambda ranges over, of the set or of the item of an
    /// enclosing `any` over a navigation: the Relationship, the target set, and the term the
    /// path is inside with its depth.
    fn navigation(&self, collection: &MemberPath) -> Option<(&'a Member, &'a Set, String, usize)> {
        let (from, open, depth) = match &collection.variable {
            None => (self.set, String::new(), 0),
            Some(v) => {
                let hop = self.hops.iter().rev().find(|h| &h.variable == v)?;
                (hop.set, hop.open.clone(), hop.depth)
            }
        };
        let [name] = collection.path.as_slice() else {
            return None;
        };
        let member = from.members.get(name)?;
        let Shape::Nav {
            target,
            collection: true,
        } = &member.shape
        else {
            return None;
        };
        Some((member, self.sets.get(target)?, open, depth))
    }

    /// T-5906: `nav/any(d: body)` over a collection-valued navigation as the linked-entity terms
    /// of TS 104 175 4.9, which hold when one target the caller may read matches (the broker
    /// resolves each object through the caller's narrowing, a hidden one like a dangling link,
    /// O26); `nav/any()` is the Relationship being present.
    fn linked_any(
        &mut self,
        collection: &MemberPath,
        lambda: Option<&(String, Box<Expr>)>,
    ) -> Result<String, Refused> {
        let (rel, target, open, depth) = self.navigation(collection).ok_or(Refused::Later)?;
        let rel = self.term_of(&rel.iri);
        let Some((variable, body)) = lambda else {
            // the Relationship itself, as deep as the enclosing hops
            self.linked = self.linked.max(depth);
            return Ok(format!("{open}{rel}{}", "}".repeat(depth)));
        };
        self.linked = self.linked.max(depth.saturating_add(1));
        let ty = self.term_of(&target.type_iri);
        self.hops.push(Hop {
            variable: variable.clone(),
            set: target,
            open: format!("{open}{rel}{{{ty}:"),
            depth: depth.saturating_add(1),
        });
        let term = self.term(body);
        self.hops.pop();
        term
    }

    /// F29: `geo.intersects(p, geography'SRID=4326;Polygon(...)')` as a
    /// top-level conjunct, `None` for any other conjunct. NGSI-LD
    /// coordinates are WGS 84 (5.2.2.4), so another SRID is not mapped.
    fn geo(&mut self, e: &Expr) -> Result<Option<(String, Value)>, Refused> {
        let Expr::Call(Method::GeoIntersects, args) = e else {
            return Ok(None);
        };
        let [member, area] = args.as_slice() else {
            return Err(Refused::Later);
        };
        let Ok(Literal::Geography(Geography {
            srid: 4326,
            shape: odata_url::Shape::Polygon(rings),
        })) = literal(area, self.aliases)
        else {
            return Err(Refused::Later);
        };
        let Expr::Member(MemberPath {
            variable: None,
            path,
        }) = member
        else {
            return Err(Refused::Later);
        };
        let Some((None, m)) = self.leaf(path) else {
            return Err(Refused::Later);
        };
        if !matches!(m.shape, Shape::Geo) {
            return Err(Refused::Bad);
        }
        let mut polygon = Vec::with_capacity(rings.len());
        for ring in rings {
            let mut points = Vec::with_capacity(ring.len());
            for position in ring {
                let point = position
                    .0
                    .iter()
                    .map(|n| {
                        n.parse::<f64>()
                            .ok()
                            .filter(|n| n.is_finite())
                            .map(Value::from)
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or(Refused::Bad)?;
                points.push(Value::from(point));
            }
            polygon.push(Value::from(points));
        }
        let term = self.term_of(&m.iri);
        Ok(Some((term, Value::from(polygon))))
    }

    /// F1 to F6, F2/F4 on `null`, F25 refused.
    fn compare(&mut self, l: &Expr, op: CompareOp, r: &Expr) -> Result<String, Refused> {
        let (member, op, lit) = match (literal(l, self.aliases), literal(r, self.aliases)) {
            (Err(_), Ok(lit)) => (l, op, lit),
            (Ok(lit), Err(_)) => (r, mirrored(op), lit),
            // F25: two members, or two literals.
            _ => return Err(Refused::Later),
        };
        if let Some(term) = self.entry(member, op, lit)? {
            return Ok(term);
        }
        let (name, edm) = self.typed_member(member)?;
        typed(edm, std::slice::from_ref(lit))?;
        Ok(match (op, lit) {
            (CompareOp::Eq, Literal::Null) => format!("!{name}"),
            (CompareOp::Ne, Literal::Null) => name,
            (_, Literal::Null) => return Err(Refused::Later),
            (CompareOp::Eq, lit) => format!("{name}=={}", value(lit)?),
            // F3: OData `ne` holds on an absent member, NGSI-LD `!=` does not.
            (CompareOp::Ne, lit) => format!("!{name}|{name}!={}", value(lit)?),
            (CompareOp::Gt, lit) => format!("{name}>{}", value(lit)?),
            (CompareOp::Ge, lit) => format!("{name}>={}", value(lit)?),
            (CompareOp::Lt, lit) => format!("{name}<{}", value(lit)?),
            (CompareOp::Le, lit) => format!("{name}<={}", value(lit)?),
        })
    }

    /// F31, F32: `eq` on a LanguageProperty, in the language the read serves
    /// (`ngsild.lang`, else the `@none` entry; T-5206 finding 6), or on a
    /// VocabProperty with its value expanded like the stored one; `None`
    /// when `member` is neither. Any other operator is a later round's.
    fn entry(
        &mut self,
        member: &Expr,
        op: CompareOp,
        lit: &Literal,
    ) -> Result<Option<String>, Refused> {
        let Expr::Member(MemberPath {
            variable: None,
            path,
        }) = member
        else {
            return Ok(None);
        };
        let Some((None, m)) = self.leaf(path) else {
            return Ok(None);
        };
        if !matches!(m.shape, Shape::Language | Shape::Vocab) {
            return Ok(None);
        }
        let (CompareOp::Eq, Literal::String(s)) = (op, lit) else {
            return Err(Refused::Later);
        };
        let term = self.term_of(&m.iri);
        let s = Value::from(s.as_str());
        Ok(Some(if matches!(m.shape, Shape::Language) {
            format!("{term}[{}]=={s}", self.lang)
        } else {
            if !self.expand_values.contains(&term) {
                self.expand_values.push(term.clone());
            }
            format!("{term}=={s}")
        }))
    }

    /// F9, F15: the literal regex-escaped, anchored for starts and ends.
    fn call(&mut self, method: Method, args: &[Expr], negated: bool) -> Result<String, Refused> {
        let [member, operand] = args else {
            return Err(Refused::Later);
        };
        let (Ok(Literal::String(s)), Method::Contains | Method::StartsWith | Method::EndsWith) =
            (literal(operand, self.aliases), method)
        else {
            return Err(Refused::Later);
        };
        let (name, edm) = self.typed_member(member)?;
        // URL 5.1.1.5: the string functions take a string; NGSI-LD would
        // match no other value without saying why.
        if edm != "Edm.String" {
            return Err(Refused::Bad);
        }
        let escaped = escape(s);
        let pattern = match method {
            Method::StartsWith => format!("^{escaped}"),
            Method::EndsWith => format!("{escaped}$"),
            _ => escaped,
        };
        let op = if negated { "!~=" } else { "~=" };
        Ok(format!("{name}{op}\"{pattern}\""))
    }

    /// The member a path ends in, and the navigation it goes through: a
    /// member of the set, or (F24) a member of the set a single-valued
    /// navigation targets.
    fn leaf(&self, path: &[String]) -> Option<(Option<(&'a Member, &'a Set)>, &'a Member)> {
        match path {
            [name] => Some((None, self.set.members.get(name)?)),
            [nav, name] => {
                let via = self.set.members.get(nav)?;
                let Shape::Nav {
                    target,
                    collection: false,
                } = &via.shape
                else {
                    return None;
                };
                let target = self.sets.get(target)?;
                Some((Some((via, target)), target.members.get(name)?))
            }
            _ => None,
        }
    }

    /// The term of a scalar member: filterable, not a collection (a
    /// collection's `eq` is membership in NGSI-LD and not in OData, F22),
    /// of the set or one hop away. A hop is `rel{Type:attr}` (7.2.3.2
    /// EXAMPLE 15); the broker resolves its target through the caller's
    /// narrowing, a hidden one like a dangling link (T-3474, O26).
    fn scalar(&mut self, e: &Expr) -> Result<String, Refused> {
        self.typed_member(e).map(|(name, _)| name)
    }

    /// [`Self::scalar`]'s term and the Edm type of its value.
    fn typed_member(&mut self, e: &Expr) -> Result<(String, &'static str), Refused> {
        let Expr::Member(MemberPath { variable, path }) = e else {
            return Err(Refused::Later);
        };
        let hop = match variable {
            None => None,
            Some(v) => Some(
                self.hops
                    .iter()
                    .rev()
                    .find(|h| &h.variable == v)
                    .ok_or(Refused::Later)?,
            ),
        };
        if let Some(hop) = hop {
            let (open, close) = (hop.open.clone(), "}".repeat(hop.depth));
            let [name] = path.as_slice() else {
                return Err(Refused::Later);
            };
            let member = hop.set.members.get(name).ok_or(Refused::Later)?;
            let edm = scalar_edm(member)?;
            let attr = self.term_of(&member.iri);
            return Ok((format!("{open}{attr}{close}"), edm));
        }
        // F21: the `observedAt` of a member of the set, as EXAMPLE 9's
        // `temperature.observedAt`.
        if let [name, annotation] = path.as_slice() {
            if annotation == OBSERVED_AT {
                let member = self.set.members.get(name).ok_or(Refused::Later)?;
                let term = format!("{}.observedAt", self.term_of(&member.iri));
                return Ok((term, "Edm.DateTimeOffset"));
            }
        }
        // The base members (`id` outside a top-level conjunct, `scope`,
        // `_types`, the system members) and complex paths (F20, F28, F33)
        // are a later round's.
        let (via, member) = self.leaf(path).ok_or(Refused::Later)?;
        let edm = scalar_edm(member)?;
        let attr = self.term_of(&member.iri);
        let term = match via {
            None => attr,
            Some((rel, target)) => {
                self.linked = self.linked.max(1);
                let rel = self.term_of(&rel.iri);
                let ty = self.term_of(&target.type_iri);
                format!("{rel}{{{ty}:{attr}}}")
            }
        };
        Ok((term, edm))
    }

    /// The sort key of `nav/name`, a sortable scalar of the target of a single-valued navigation.
    fn linked_key(&mut self, (nav, name): (&String, &String)) -> Result<SortBy, Refused> {
        let Some((Some((rel, _)), member)) = self.leaf(&[nav.clone(), name.clone()]) else {
            return Err(Refused::Later);
        };
        if !member.shape.sortable() {
            return Err(Refused::Bad);
        }
        scalar_edm(member)?;
        self.linked = self.linked.max(1);
        let rel = self.term_of(&rel.iri);
        Ok(SortBy::Linked(rel, self.term_of(&member.iri)))
    }

    /// The term `t{n}` of an IRI, minted on first use.
    fn term_of(&mut self, iri: &str) -> String {
        mint(self.context, iri)
    }

    /// One boolean expression as a residual one (ADR-0028 rule 2): the
    /// constructs `term` maps, any `not`, with OData's null rules; what the
    /// evaluator does not cover yet is `Later` (T-3749).
    fn residual(&mut self, e: &Expr) -> Result<RExpr, Refused> {
        match e {
            Expr::And(l, r) => Ok(RExpr::And(vec![self.residual(l)?, self.residual(r)?])),
            Expr::Or(l, r) => Ok(RExpr::Or(vec![self.residual(l)?, self.residual(r)?])),
            Expr::Not(inner) => Ok(RExpr::Not(Box::new(self.residual(inner)?))),
            Expr::Compare(l, op, r) => self.residual_compare(l, *op, r),
            Expr::In(l, list) => {
                let (value, edm) = self.residual_value(l)?;
                typed(edm, list)?;
                let values = list.iter().map(residual_lit).collect::<Result<_, _>>()?;
                Ok(definite(RExpr::In(Box::new(value), values)))
            }
            Expr::Call(Method::GeoIntersects, _) => self
                .geo(e)?
                .map(|(term, polygon)| intersects(&term, &polygon))
                .ok_or(Refused::Later),
            // A boolean function, a Boolean member (null when absent) or
            // `true`: URL 5.1.1.5, null in, null out, `not` keeps it null.
            Expr::Call(..) | Expr::Member(_) | Expr::Literal(_) | Expr::Alias(_) => {
                match self.residual_value(e)? {
                    (value, "Edm.Boolean") => Ok(value),
                    (_, edm) if KNOWN.contains(&edm) => Err(Refused::Bad),
                    _ => Err(Refused::Later),
                }
            }
            // F23: a lambda over a list Property, its variable a `Var`. An
            // absent list is the empty collection the surface shows: no
            // member for `any`, every member for `all`.
            Expr::Any { collection, lambda } => {
                let (list, item) = self.residual_list(collection)?;
                let body = match lambda {
                    None => None,
                    Some((variable, body)) => Some(Box::new(self.lambda(variable, item, body)?)),
                };
                let absent = RExpr::IsNull(Box::new(list.clone()));
                Ok(RExpr::And(vec![
                    RExpr::Not(Box::new(absent)),
                    RExpr::Any(Box::new(list), body),
                ]))
            }
            Expr::All {
                collection,
                lambda: (variable, body),
            } => {
                let (list, item) = self.residual_list(collection)?;
                let body = self.lambda(variable, item, body)?;
                let absent = RExpr::IsNull(Box::new(list.clone()));
                Ok(RExpr::Or(vec![
                    absent,
                    RExpr::All(Box::new(list), Box::new(body)),
                ]))
            }
            // F14: a number is no predicate.
            Expr::Arith(..) | Expr::Neg(_) => Err(Refused::Bad),
        }
    }

    /// `body` with `variable` bound to an item of type `item`.
    fn lambda(
        &mut self,
        variable: &str,
        item: &'static str,
        body: &Expr,
    ) -> Result<RExpr, Refused> {
        self.lambdas.push((variable.to_owned(), item));
        let body = self.residual(body);
        self.lambdas.pop();
        body
    }

    /// The list value of a collection member of the set, and its items'
    /// Edm type; a member that is no list is 400, a hop or another shape
    /// a later round's.
    fn residual_list(&mut self, collection: &MemberPath) -> Result<(RExpr, &'static str), Refused> {
        let MemberPath {
            variable: None,
            path,
        } = collection
        else {
            return Err(Refused::Later);
        };
        let Some((None, member)) = self.leaf(path) else {
            return Err(Refused::Later);
        };
        match &member.shape {
            Shape::Value {
                edm,
                collection: true,
            } => Ok((RExpr::Path(Path::value(&self.term_of(&member.iri))), *edm)),
            Shape::Value { .. } | Shape::Json => Err(Refused::Bad),
            _ => Err(Refused::Later),
        }
    }

    /// A value expression as a residual one and its Edm type (F14, F16 to
    /// F19, F23, F25): a literal, a member, a lambda variable, a function
    /// of the closed table, arithmetic. An argument of a known type the
    /// function does not take is 400 (URL 5.1.1); `null` as a value, a
    /// duration and what the evaluator has no function for are `Later`.
    fn residual_value(&mut self, e: &Expr) -> Result<(RExpr, &'static str), Refused> {
        match e {
            Expr::Literal(_) | Expr::Alias(_) => {
                let lit = literal(e, self.aliases)?;
                let edm = literal_edm(lit).ok_or(Refused::Later)?;
                Ok((RExpr::Lit(residual_lit(lit)?), edm))
            }
            Expr::Member(MemberPath {
                variable: Some(variable),
                path,
            }) => {
                let (up, (_, item)) = self
                    .lambdas
                    .iter()
                    .rev()
                    .enumerate()
                    .find(|(_, (v, _))| v == variable)
                    .ok_or(Refused::Bad)?;
                let item = *item;
                if !path.is_empty() {
                    return Err(Refused::Later);
                }
                Ok((
                    RExpr::Var {
                        up,
                        keys: Vec::new(),
                    },
                    item,
                ))
            }
            Expr::Member(MemberPath {
                variable: None,
                path,
            }) => {
                let (path, edm) = self.residual_member(path)?;
                Ok((RExpr::Path(path), edm))
            }
            Expr::Call(method, args) => {
                let (func, params, result) = signature(*method).ok_or(Refused::Later)?;
                let (min, max) = method.arity();
                if !(min..=max).contains(&args.len()) {
                    return Err(Refused::Bad);
                }
                let mut values = Vec::with_capacity(args.len());
                let mut first = None;
                for (i, arg) in args.iter().enumerate() {
                    let (value, edm) = self.residual_value(arg)?;
                    let takes = params.get(i).or(params.last()).copied().unwrap_or(&[]);
                    takes_arg(takes, edm)?;
                    first.get_or_insert(edm);
                    values.push(value);
                }
                let edm = result.or(first).unwrap_or("Edm.Decimal");
                Ok((RExpr::Call(func, values), edm))
            }
            Expr::Arith(l, op, r) => {
                let (l, le) = self.residual_value(l)?;
                let (r, re) = self.residual_value(r)?;
                takes_arg(NUMBER, le)?;
                takes_arg(NUMBER, re)?;
                let integer = family(le) == "integer" && family(re) == "integer";
                let (func, edm) = match op {
                    ArithOp::Add => (Func::Add, integer),
                    ArithOp::Sub => (Func::Sub, integer),
                    ArithOp::Mul => (Func::Mul, integer),
                    ArithOp::Div => (Func::Div, integer),
                    ArithOp::DivBy => (Func::DivBy, false),
                    ArithOp::Mod => (Func::Mod, integer),
                };
                let edm = if edm { "Edm.Int64" } else { "Edm.Decimal" };
                Ok((RExpr::Call(func, vec![l, r]), edm))
            }
            Expr::Neg(operand) => {
                let (value, edm) = self.residual_value(operand)?;
                takes_arg(NUMBER, edm)?;
                Ok((RExpr::Call(Func::Neg, vec![value]), edm))
            }
            Expr::Compare(..) | Expr::In(..) | Expr::And(..) | Expr::Or(..) | Expr::Not(_) => {
                Ok((self.residual(e)?, "Edm.Boolean"))
            }
            Expr::Any { .. } | Expr::All { .. } => Ok((self.residual(e)?, "Edm.Boolean")),
        }
    }

    /// F1 to F6 and F2/F4 as residual comparisons: definite, so a missing
    /// value or one of another type compares false (URL 5.1.1.1.2, 5.1.1.1.3)
    /// and `ne` holds on it.
    fn residual_compare(&mut self, l: &Expr, op: CompareOp, r: &Expr) -> Result<RExpr, Refused> {
        let split = match (
            plain(l),
            literal(r, self.aliases),
            plain(r),
            literal(l, self.aliases),
        ) {
            (Some(member), Ok(lit), ..) => Some((member, op, lit)),
            (.., Some(member), Ok(lit)) => Some((member, mirrored(op), lit)),
            _ => None,
        };
        // F25 and F14: two values of any kind.
        let Some((member, op, lit)) = split else {
            return self.residual_values(l, op, r);
        };
        let (path, edm) = self.residual_member(member)?;
        typed(edm, std::slice::from_ref(lit))?;
        let value = || RExpr::Path(path.clone());
        let null = || RExpr::IsNull(Box::new(value()));
        if matches!(lit, Literal::Null) {
            return match op {
                CompareOp::Eq => Ok(null()),
                CompareOp::Ne => Ok(RExpr::Not(Box::new(null()))),
                _ => Err(Refused::Later),
            };
        }
        let lit = residual_lit(lit)?;
        let cmp = |op| RExpr::Cmp(op, Box::new(value()), Box::new(RExpr::Lit(lit.clone())));
        Ok(match op {
            CompareOp::Eq => definite(cmp(Cmp::Eq)),
            CompareOp::Ne => RExpr::Not(Box::new(definite(cmp(Cmp::Eq)))),
            CompareOp::Gt => definite(cmp(Cmp::Gt)),
            CompareOp::Ge => definite(cmp(Cmp::Ge)),
            CompareOp::Lt => definite(cmp(Cmp::Lt)),
            CompareOp::Le => definite(cmp(Cmp::Le)),
        })
    }

    /// `l op r` over two values: of one family (numbers promote, 5.1.1.1),
    /// definite like any comparison, `null` on either side the value's
    /// absence.
    fn residual_values(&mut self, l: &Expr, op: CompareOp, r: &Expr) -> Result<RExpr, Refused> {
        let null = |e: &Expr| matches!(literal(e, self.aliases), Ok(Literal::Null));
        if null(l) || null(r) {
            let other = if null(l) { r } else { l };
            let absent = RExpr::IsNull(Box::new(self.residual_value(other)?.0));
            return match op {
                CompareOp::Eq => Ok(absent),
                CompareOp::Ne => Ok(RExpr::Not(Box::new(absent))),
                _ => Err(Refused::Later),
            };
        }
        let (lv, le) = self.residual_value(l)?;
        let (rv, re) = self.residual_value(r)?;
        let ordered = !matches!(op, CompareOp::Eq | CompareOp::Ne);
        let comparable = family(le) == family(re)
            || NUMBER.contains(&family(le)) && NUMBER.contains(&family(re));
        if !comparable && KNOWN.contains(&le) && KNOWN.contains(&re)
            || ordered && le == "Edm.Boolean"
        {
            return Err(Refused::Bad);
        }
        let cmp = |c| definite(RExpr::Cmp(c, Box::new(lv.clone()), Box::new(rv.clone())));
        Ok(match op {
            CompareOp::Eq => cmp(Cmp::Eq),
            CompareOp::Ne => RExpr::Not(Box::new(cmp(Cmp::Eq))),
            CompareOp::Gt => cmp(Cmp::Gt),
            CompareOp::Ge => cmp(Cmp::Ge),
            CompareOp::Lt => cmp(Cmp::Lt),
            CompareOp::Le => cmp(Cmp::Le),
        })
    }

    /// [`Self::typed_member`] as a residual path: a scalar member of the set
    /// or its `observedAt` (F21); a hop is gated (F24) and stays `Later`.
    fn residual_member(&mut self, path: &[String]) -> Result<(Path, &'static str), Refused> {
        if let [name, annotation] = path {
            if annotation == OBSERVED_AT {
                let member = self.set.members.get(name).ok_or(Refused::Later)?;
                let term = self.term_of(&member.iri);
                return Ok((
                    Path::Attribute {
                        attrs: vec![term],
                        member: RMember::ObservedAt,
                        keys: Vec::new(),
                    },
                    "Edm.DateTimeOffset",
                ));
            }
        }
        let Some((None, member)) = self.leaf(path) else {
            return Err(Refused::Later);
        };
        let edm = match &member.shape {
            Shape::Value {
                edm,
                collection: false,
            } => *edm,
            // T-5333: an Untyped Property compares by its JSON value, as `q` does.
            Shape::Json if member.untyped => "Edm.Untyped",
            Shape::Json => return Err(Refused::Bad),
            _ => return Err(Refused::Later),
        };
        Ok((Path::value(&self.term_of(&member.iri)), edm))
    }
}

/// The Edm type a member compares as: the shapes `Shape::scalar` names, the test the model's
/// `NonFilterableProperties` and `NonSortableProperties` are written from (C11, C12).
fn scalar_edm(member: &Member) -> Result<&'static str, Refused> {
    match &member.shape {
        Shape::Value {
            edm,
            collection: false,
        } => Ok(*edm),
        // T-5333: an Untyped Property compares by its JSON value, as `q` does.
        Shape::Json if member.untyped => Ok("Edm.Untyped"),
        Shape::Json => Err(Refused::Bad),
        _ => Err(Refused::Later),
    }
}

/// URL 5.1.1.1: a literal compared with a member of type `edm` is of that
/// type, an integer within its range (T-3725); `null` compares with any.
fn typed(edm: &str, literals: &[Literal]) -> Result<(), Refused> {
    if literals.iter().all(|l| fits(edm, l)) {
        Ok(())
    } else {
        Err(Refused::Bad)
    }
}

/// The path of a member of the set, not of a lambda variable.
fn plain(e: &Expr) -> Option<&[String]> {
    match e {
        Expr::Member(MemberPath {
            variable: None,
            path,
        }) => Some(path),
        _ => None,
    }
}

/// The Edm types this surface checks; a value of another type is taken
/// where a function asks (as [`fits`] takes any literal for it).
const KNOWN: [&str; 16] = [
    "Edm.String",
    "Edm.Boolean",
    "Edm.Guid",
    "Edm.Date",
    "Edm.DateTimeOffset",
    "Edm.TimeOfDay",
    "Edm.Byte",
    "Edm.SByte",
    "Edm.Int16",
    "Edm.Int32",
    "Edm.Int64",
    "Edm.Single",
    "Edm.Double",
    "Edm.Decimal",
    "Edm.Duration",
    "Edm.Binary",
];

/// An Edm type's family: the integer types, the other numbers, or itself.
fn family(edm: &str) -> &str {
    match edm {
        "Edm.Byte" | "Edm.SByte" | "Edm.Int16" | "Edm.Int32" | "Edm.Int64" => "integer",
        "Edm.Single" | "Edm.Double" | "Edm.Decimal" => "number",
        other => other,
    }
}

/// 400 when `edm` is a type this surface knows and of no family `takes`.
fn takes_arg(takes: &[&str], edm: &str) -> Result<(), Refused> {
    if !KNOWN.contains(&edm) || takes.contains(&family(edm)) {
        Ok(())
    } else {
        Err(Refused::Bad)
    }
}

/// The Edm type of a literal; `None` for `null`.
fn literal_edm(lit: &Literal) -> Option<&'static str> {
    Some(match lit {
        Literal::Null => return None,
        Literal::Bool(_) => "Edm.Boolean",
        Literal::Int(_) => "Edm.Int64",
        Literal::Decimal(_) => "Edm.Decimal",
        Literal::String(_) => "Edm.String",
        Literal::Guid(_) => "Edm.Guid",
        Literal::Date(_) => "Edm.Date",
        Literal::DateTimeOffset(_) => "Edm.DateTimeOffset",
        Literal::TimeOfDay(_) => "Edm.TimeOfDay",
        Literal::Geography(_) => "Edm.Geography",
        Literal::Duration(_) => "Edm.Duration",
        Literal::Binary(_) => "Edm.Binary",
    })
}

/// Whether `lit` is a value of `edm` (T-3725): numbers promote among the
/// numeric types, but an integer literal past an integer type's range does
/// not (5.1.1.5); a type this surface does not know takes any literal.
fn fits(edm: &str, lit: &Literal) -> bool {
    let range = |min: i64, max: i64| match lit {
        Literal::Int(n) => (min..=max).contains(n),
        // An integer written past i64 reads as a decimal: no integer type holds it.
        Literal::Decimal(raw) => !raw
            .trim_start_matches(['-', '+'])
            .bytes()
            .all(|b| b.is_ascii_digit()),
        _ => false,
    };
    let same = matches!(
        (edm, lit),
        (_, Literal::Null)
            | ("Edm.String", Literal::String(_))
            | ("Edm.Boolean", Literal::Bool(_))
            | ("Edm.Guid", Literal::Guid(_))
            | ("Edm.Date", Literal::Date(_))
            | ("Edm.DateTimeOffset", Literal::DateTimeOffset(_))
            | ("Edm.TimeOfDay", Literal::TimeOfDay(_))
            | ("Edm.Duration", Literal::Duration(_))
            | ("Edm.Binary", Literal::Binary(_))
            | (
                "Edm.Single" | "Edm.Double" | "Edm.Decimal",
                Literal::Int(_) | Literal::Decimal(_)
            )
            // A JSON value: never a date, GUID or geography (T-5333).
            | (
                "Edm.Untyped",
                Literal::Bool(_) | Literal::Int(_) | Literal::Decimal(_) | Literal::String(_)
            )
    );
    match edm {
        // a finite decimal past the `Edm.Single` range is no Single (5.1.1.5)
        "Edm.Single" if same => !matches!(lit, Literal::Decimal(raw)
            if raw.parse::<f64>().is_ok_and(|n| n.is_finite() && n.abs() > f64::from(f32::MAX))),
        _ if same => true,
        "Edm.Byte" => range(0, 255),
        "Edm.SByte" => range(-128, 127),
        "Edm.Int16" => range(i16::MIN.into(), i16::MAX.into()),
        "Edm.Int32" => range(i32::MIN.into(), i32::MAX.into()),
        "Edm.Int64" => range(i64::MIN, i64::MAX),
        "Edm.String" | "Edm.Boolean" | "Edm.Guid" | "Edm.Date" | "Edm.DateTimeOffset"
        | "Edm.TimeOfDay" | "Edm.Single" | "Edm.Double" | "Edm.Decimal" | "Edm.Untyped"
        | "Edm.Duration" | "Edm.Binary" => false,
        _ => true,
    }
}

/// The term `t{n}` of `iri` in `context`, minted on first use.
pub(crate) fn mint(context: &mut Map<String, Value>, iri: &str) -> String {
    let found = context
        .iter()
        .find(|(_, defined)| defined.as_str() == Some(iri))
        .map(|(term, _)| term.clone());
    found.unwrap_or_else(|| {
        let next = format!("t{}", context.len());
        context.insert(next.clone(), Value::from(iri));
        next
    })
}

/// `e`, with unknown read as false: `not (e is null) and e` (OData's
/// comparisons are two-valued, the evaluator's three-valued).
fn definite(e: RExpr) -> RExpr {
    RExpr::And(vec![
        RExpr::Not(Box::new(RExpr::IsNull(Box::new(e.clone())))),
        e,
    ])
}

/// F29 as a residual conjunct: the GeoProperty of `term` intersects
/// `polygon` (`coordinates` of a WGS 84 Polygon).
fn intersects(term: &str, polygon: &Value) -> RExpr {
    RExpr::Spatial(
        Spatial::Intersects,
        Box::new(RExpr::Path(Path::value(term))),
        Box::new(RExpr::Lit(Lit::Geometry(serde_json::json!({
            "type": "Polygon",
            "coordinates": polygon,
        })))),
    )
}

/// A literal as a residual value: an offset date-time is one instant, which
/// `q` cannot say (F26).
fn residual_lit(lit: &Literal) -> Result<Lit, Refused> {
    let parsed = |v: Option<Lit>| v.ok_or(Refused::Bad);
    match lit {
        Literal::Bool(b) => Ok(Lit::Bool(*b)),
        Literal::Int(n) => Ok(Lit::Int(*n)),
        Literal::Decimal(raw) => match (raw.as_str(), raw.parse::<f64>()) {
            ("INF", _) => Ok(Lit::Num(f64::INFINITY)),
            ("-INF", _) => Ok(Lit::Num(f64::NEG_INFINITY)),
            ("NaN", _) => Ok(Lit::Num(f64::NAN)),
            (_, Ok(n)) if n.is_finite() => Ok(Lit::Num(n)),
            _ => Err(Refused::Bad),
        },
        Literal::String(s) | Literal::Guid(s) => Ok(Lit::Str(s.clone())),
        Literal::Duration(d) => parsed(Duration::parse(d).map(Lit::Duration)),
        Literal::Binary(b) => parsed(decode_base64(b).map(Lit::Binary)),
        Literal::Date(d) => parsed(Date::parse(d).map(Lit::Date)),
        Literal::DateTimeOffset(t) => parsed(DateTime::parse(t).map(Lit::DateTime)),
        Literal::TimeOfDay(t) => parsed(Time::parse(t).map(Lit::Time)),
        Literal::Null | Literal::Geography(_) => Err(Refused::Later),
    }
}

/// The residual of `t` as one program, `None` when the filter pushes whole.
///
/// # Errors
/// The evaluator's bounds (ADR-0028 rule 7) as `TooComplex`, anything it
/// does not take as `Later`.
pub(crate) fn program(t: &Translated, now: DateTime) -> Result<Option<residual::Program>, Refused> {
    let conjuncts = match t.residual.as_slice() {
        [] => return Ok(None),
        [one] => one.clone(),
        many => RExpr::And(many.to_vec()),
    };
    compiled(&conjuncts, now).map(Some)
}

/// The residual of `t` as one program that holds for every Entity when `t` has none: a read
/// of the whole answer.
///
/// # Errors
/// As [`program`].
pub(crate) fn whole(t: &Translated, now: DateTime) -> Result<residual::Program, Refused> {
    compiled(&RExpr::And(t.residual.clone()), now)
}

/// Whether the Edm type `edm` is a number's.
pub(crate) fn numeric(edm: &str) -> bool {
    takes_arg(NUMBER, edm).is_ok()
}

/// `e` compiled under the evaluator's bounds.
fn compiled(e: &RExpr, now: DateTime) -> Result<residual::Program, Refused> {
    residual::compile(e, now).map_err(|e| match e {
        residual::Error::TooComplex => Refused::TooComplex,
        // this module builds no pattern, the one construct the evaluator may not support
        residual::Error::Invalid(_) | residual::Error::Unsupported(_) => Refused::Bad,
    })
}

/// `v op p` read as `p op' v`.
fn mirrored(op: CompareOp) -> CompareOp {
    match op {
        CompareOp::Gt => CompareOp::Lt,
        CompareOp::Ge => CompareOp::Le,
        CompareOp::Lt => CompareOp::Gt,
        CompareOp::Le => CompareOp::Ge,
        same => same,
    }
}

/// A literal as a `q` value (F26, F27).
fn value(lit: &Literal) -> Result<String, Refused> {
    match lit {
        Literal::Bool(b) => Ok(b.to_string()),
        Literal::Int(n) if (-MAX_EXACT..=MAX_EXACT).contains(n) => Ok(n.to_string()),
        Literal::Int(_) => Err(Refused::Bad),
        // INF, -INF and NaN have no NGSI-LD number: the residual compares them (T-5905)
        Literal::Decimal(raw) if matches!(raw.as_str(), "INF" | "-INF" | "NaN") => {
            Err(Refused::Later)
        }
        Literal::Decimal(raw) => match raw.parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(Value::from(n).to_string()),
            _ => Err(Refused::Bad),
        },
        // An NGSI-LD Property holds a GUID as its string (T-3720).
        Literal::String(s) | Literal::Guid(s) => Ok(Value::from(s.as_str()).to_string()),
        Literal::Date(d) => Ok(d.clone()),
        Literal::DateTimeOffset(t) if t.ends_with('Z') => Ok(t.clone()),
        // An offset other than Z, null in a list, a geography: later rounds. A time of day, a
        // duration and bytes NGSI-LD stores as text the q string order does not compare
        // (`09:30:00` and `09:30:00Z`, `PT2H` and `P1D`, padded and unpadded base64): the
        // residual compares the values they spell (T-5905).
        Literal::DateTimeOffset(_)
        | Literal::Null
        | Literal::Geography(_)
        | Literal::TimeOfDay(_)
        | Literal::Duration(_)
        | Literal::Binary(_) => Err(Refused::Later),
    }
}

/// The characters of `s` with every regular-expression metacharacter
/// escaped, and `"` as `\x22`: the quoted `RegExp` of `q` is taken as
/// written up to the next quote.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '"' {
            out.push_str(r"\x22");
            continue;
        }
        if r"\.^$|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::functions::STRING;

    fn translate(
        filter: &Expr,
        set: &Set,
        sets: &BTreeMap<String, Set>,
        aliases: &[(String, Literal)],
    ) -> Result<Translated, Refused> {
        super::translate(filter, set, sets, aliases, None)
    }
    use crate::edm::Member;
    use std::collections::BTreeMap;

    fn set() -> Set {
        let member = |iri: &str, shape| Member {
            iri: iri.into(),
            shape,
            list: false,
            json: false,
            untyped: false,
            written: crate::edm::Written::Always,
        };
        let value = |edm, collection| Shape::Value { edm, collection };
        Set {
            output: None,
            type_iri: "https://e.org/Car".into(),
            identifier: None,
            references: BTreeMap::new(),
            bound: BTreeMap::new(),
            writable: crate::edm::Writable::default(),
            members: BTreeMap::from([
                (
                    "speed".into(),
                    member("https://e.org/speed", value("Edm.Double", false)),
                ),
                (
                    "name".into(),
                    member("https://e.org/name", value("Edm.String", false)),
                ),
                (
                    "on".into(),
                    member("https://e.org/on", value("Edm.Boolean", false)),
                ),
                (
                    "tags".into(),
                    member("https://e.org/tags", value("Edm.String", true)),
                ),
                ("doc".into(), member("https://e.org/doc", Shape::Json)),
                // T-5333: `range: Any`, a number or the string "not measured"
                (
                    "reading".into(),
                    Member {
                        untyped: true,
                        ..member("https://e.org/reading", Shape::Json)
                    },
                ),
                // a type this surface does not check
                (
                    "span".into(),
                    member("https://e.org/span", value("Edm.Duration", false)),
                ),
                // a type this surface does not check
                (
                    "clip".into(),
                    member("https://e.org/clip", value("Edm.Stream", false)),
                ),
                (
                    "label".into(),
                    member("https://e.org/label", Shape::Language),
                ),
                (
                    "status".into(),
                    member("https://e.org/status", Shape::Vocab),
                ),
                (
                    "location".into(),
                    member("https://e.org/location", Shape::Geo),
                ),
                (
                    "owner".into(),
                    member("https://e.org/owner", Shape::Url { collection: false }),
                ),
            ]),
        }
    }

    fn m(name: &str) -> Expr {
        Expr::Member(MemberPath {
            variable: None,
            path: vec![name.into()],
        })
    }

    fn lit(l: Literal) -> Expr {
        Expr::Literal(l)
    }

    fn cmp(l: Expr, op: CompareOp, r: Expr) -> Expr {
        Expr::Compare(Box::new(l), op, Box::new(r))
    }

    fn q(e: &Expr) -> Result<String, Refused> {
        translate(
            e,
            &set(),
            &BTreeMap::new(),
            &[("a".into(), Literal::Int(3))],
        )
        .map(|t| t.q.unwrap_or_default())
    }

    /// T-4919: a key the filter holds needs no count of the Entities lacking
    /// it; `ne`, `eq null`, `not`, an alias and a one-sided `or` do not hold it.
    #[test]
    fn a_comparison_with_a_literal_holds_its_member() {
        let and = |l: Expr, r: Expr| Expr::And(Box::new(l), Box::new(r));
        let or = |l: Expr, r: Expr| Expr::Or(Box::new(l), Box::new(r));
        let ge = |n: &str| cmp(m(n), CompareOp::Ge, lit(Literal::Int(30)));
        let range = and(
            ge("level"),
            cmp(m("level"), CompareOp::Lt, lit(Literal::Int(40))),
        );
        assert!(holds_member(&range, "level"));
        assert!(!holds_member(&range, "speed"));
        assert!(holds_member(&and(ge("speed"), ge("level")), "level"));
        assert!(holds_member(
            &cmp(lit(Literal::Int(1)), CompareOp::Le, m("level")),
            "level"
        ));
        assert!(holds_member(&or(ge("level"), ge("level")), "level"));
        assert!(!holds_member(&or(ge("level"), ge("speed")), "level"));
        assert!(!holds_member(
            &cmp(m("level"), CompareOp::Ne, lit(Literal::Int(3))),
            "level"
        ));
        assert!(!holds_member(
            &cmp(m("level"), CompareOp::Eq, lit(Literal::Null)),
            "level"
        ));
        assert!(!holds_member(
            &cmp(m("level"), CompareOp::Gt, Expr::Alias("a".into())),
            "level"
        ));
        assert!(!holds_member(&Expr::Not(Box::new(ge("level"))), "level"));
    }

    #[test]
    fn comparisons_mirror_and_name_their_terms() {
        let speed = |op, n| cmp(m("speed"), op, lit(Literal::Int(n)));
        assert_eq!(q(&speed(CompareOp::Eq, 1)), Ok("(t0==1)".into()));
        assert_eq!(q(&speed(CompareOp::Gt, 1)), Ok("(t0>1)".into()));
        assert_eq!(q(&speed(CompareOp::Ge, 1)), Ok("(t0>=1)".into()));
        assert_eq!(q(&speed(CompareOp::Lt, 1)), Ok("(t0<1)".into()));
        assert_eq!(q(&speed(CompareOp::Le, 1)), Ok("(t0<=1)".into()));
        assert_eq!(
            q(&speed(CompareOp::Ne, 1)),
            Ok("(!t0|t0!=1)".into()),
            "F3: ne holds on the absent member"
        );
        for (op, want) in [
            (CompareOp::Gt, "<"),
            (CompareOp::Ge, "<="),
            (CompareOp::Lt, ">"),
            (CompareOp::Le, ">="),
            (CompareOp::Eq, "=="),
        ] {
            let e = cmp(lit(Literal::Int(1)), op, m("speed"));
            assert_eq!(q(&e), Ok(format!("(t0{want}1)")), "{op:?}");
        }
        let alias = cmp(m("speed"), CompareOp::Lt, Expr::Alias("a".into()));
        assert_eq!(q(&alias), Ok("(t0<3)".into()));
        let unbound = cmp(m("speed"), CompareOp::Lt, Expr::Alias("b".into()));
        assert_eq!(
            q(&unbound),
            Err(Refused::Bad),
            "an unbound alias is no value"
        );
    }

    #[test]
    fn and_or_group_and_one_iri_is_one_term() {
        let e = Expr::And(
            Box::new(cmp(m("speed"), CompareOp::Gt, lit(Literal::Int(1)))),
            Box::new(Expr::Or(
                Box::new(cmp(
                    m("name"),
                    CompareOp::Eq,
                    lit(Literal::String("a\"b".into())),
                )),
                Box::new(cmp(
                    m("speed"),
                    CompareOp::Lt,
                    lit(Literal::Decimal("0.5".into())),
                )),
            )),
        );
        let t = translate(&e, &set(), &BTreeMap::new(), &[]).unwrap_or_default();
        assert_eq!(t.q.as_deref(), Some(r#"(t0>1);((t1=="a\"b")|(t0<0.5))"#));
        assert_eq!(t.context.len(), 2);
        assert_eq!(t.context["t1"], "https://e.org/name");
        assert_eq!(t.ids, None);
        let nested = Expr::Or(
            Box::new(m("on")),
            Box::new(Expr::And(Box::new(m("on")), Box::new(m("on")))),
        );
        assert_eq!(
            q(&nested),
            Ok("((t0==true)|((t0==true);(t0==true)))".into())
        );
    }

    #[test]
    fn null_in_and_flags() {
        let null = |op| cmp(m("name"), op, lit(Literal::Null));
        assert_eq!(q(&null(CompareOp::Eq)), Ok("(!t0)".into()));
        assert_eq!(q(&null(CompareOp::Ne)), Ok("(t0)".into()));
        assert_eq!(
            q(&Expr::Not(Box::new(null(CompareOp::Eq)))),
            Ok("(t0)".into())
        );
        assert_eq!(q(&null(CompareOp::Gt)), Err(Refused::Later));
        let within = Expr::In(
            Box::new(m("speed")),
            vec![Literal::Int(1), Literal::Decimal("2.5".into())],
        );
        assert_eq!(q(&within), Ok("(t0==1,2.5)".into()));
        assert_eq!(
            q(&Expr::In(Box::new(m("speed")), vec![Literal::Null])),
            Err(Refused::Later)
        );
        assert_eq!(q(&m("on")), Ok("(t0==true)".into()));
    }

    #[test]
    fn a_path_past_one_hop_is_later() {
        let deep = Expr::Member(MemberPath {
            variable: None,
            path: vec!["nav".into(), "name".into(), "x".into()],
        });
        assert_eq!(
            q(&cmp(deep, CompareOp::Eq, lit(Literal::Int(1)))),
            Err(Refused::Later)
        );
    }

    #[test]
    fn patterns_are_escaped_and_anchored() {
        let call =
            |method, s: &str| Expr::Call(method, vec![m("name"), lit(Literal::String(s.into()))]);
        assert_eq!(
            q(&call(Method::Contains, "a.b")),
            Ok(r#"(t0~="a\.b")"#.into())
        );
        assert_eq!(
            q(&call(Method::StartsWith, "(x")),
            Ok(r#"(t0~="^\(x")"#.into())
        );
        assert_eq!(q(&call(Method::EndsWith, "$")), Ok(r#"(t0~="\$$")"#.into()));
        let not = Expr::Not(Box::new(call(Method::Contains, "x")));
        assert_eq!(q(&not), Ok(r#"(t0!~="x")"#.into()), "F9");
        assert_eq!(
            q(&call(Method::Contains, "a\"b\\")),
            Ok(r#"(t0~="a\x22b\\")"#.into())
        );
        assert_eq!(q(&call(Method::GeoIntersects, "x")), Err(Refused::Later));
        let one_arg = Expr::Call(Method::Contains, vec![m("name")]);
        assert_eq!(q(&one_arg), Err(Refused::Bad));
        let number = Expr::Call(Method::Contains, vec![m("name"), lit(Literal::Int(1))]);
        assert_eq!(
            q(&number),
            Err(Refused::Bad),
            "a number where a string goes"
        );
        let on_a_number = Expr::Call(
            Method::Contains,
            vec![m("speed"), lit(Literal::String("1".into()))],
        );
        assert_eq!(
            q(&on_a_number),
            Err(Refused::Bad),
            "a string function on a Double"
        );
    }

    #[test]
    fn ids_are_a_top_level_conjunct() {
        let id = |l: &str| cmp(m("id"), CompareOp::Eq, lit(Literal::String(l.into())));
        let t = translate(&id("urn:a"), &set(), &BTreeMap::new(), &[]).unwrap_or_default();
        assert_eq!((t.q, t.ids), (None, Some(vec!["urn:a".to_owned()])));
        let mirrored = cmp(lit(Literal::String("urn:b".into())), CompareOp::Eq, m("id"));
        let both = Expr::And(
            Box::new(Expr::In(
                Box::new(m("id")),
                vec![Literal::String("urn:a".into())],
            )),
            Box::new(cmp(m("speed"), CompareOp::Gt, lit(Literal::Int(0)))),
        );
        let t = translate(&both, &set(), &BTreeMap::new(), &[]).unwrap_or_default();
        assert_eq!(
            (t.q.as_deref(), t.ids),
            (Some("(t0>0)"), Some(vec!["urn:a".to_owned()]))
        );
        // one `id` parameter per query: the second list is evaluated (T-3758)
        let twice = Expr::And(Box::new(id("urn:a")), Box::new(mirrored));
        let t = translate(&twice, &set(), &BTreeMap::new(), &[]).unwrap_or_default();
        assert_eq!(t.ids, Some(vec!["urn:a".to_owned()]));
        assert_eq!(
            t.residual,
            [RExpr::In(
                Box::new(RExpr::Path(Path::Id)),
                vec![Lit::Str("urn:b".into())]
            )]
        );
        let number = cmp(m("id"), CompareOp::Eq, lit(Literal::Int(1)));
        assert_eq!(
            translate(&number, &set(), &BTreeMap::new(), &[]),
            Err(Refused::Bad)
        );
        let nested = Expr::Or(Box::new(id("urn:a")), Box::new(id("urn:b")));
        assert_eq!(
            translate(&nested, &set(), &BTreeMap::new(), &[]),
            Err(Refused::Later),
            "F28: top level only"
        );
        let unbound = cmp(m("id"), CompareOp::Eq, Expr::Alias("x".into()));
        assert_eq!(
            translate(&unbound, &set(), &BTreeMap::new(), &[]),
            Err(Refused::Bad)
        );
    }

    #[test]
    fn what_this_round_does_not_map_is_refused() {
        let later = [
            cmp(m("tags"), CompareOp::Eq, lit(Literal::String("a".into()))),
            cmp(
                m("location"),
                CompareOp::Eq,
                lit(Literal::String("a".into())),
            ),
            cmp(m("owner"), CompareOp::Eq, lit(Literal::String("a".into()))),
            cmp(m("scope"), CompareOp::Eq, lit(Literal::String("/a".into()))),
            cmp(
                Expr::Member(MemberPath {
                    variable: None,
                    path: vec!["speed".into(), "x".into()],
                }),
                CompareOp::Eq,
                lit(Literal::Int(1)),
            ),
        ];
        for e in later {
            assert_eq!(q(&e), Err(Refused::Later), "{e:?}");
        }
        for e in [
            cmp(
                Expr::Member(MemberPath {
                    variable: Some("x".into()),
                    path: vec!["speed".into()],
                }),
                CompareOp::Eq,
                lit(Literal::Int(1)),
            ),
            cmp(m("doc"), CompareOp::Eq, lit(Literal::Int(1))),
            cmp(m("speed"), CompareOp::Eq, lit(Literal::Int(MAX_EXACT + 1))),
        ] {
            assert_eq!(q(&e), Err(Refused::Bad), "{e:?}");
        }
    }

    fn call_of(method: Method, args: Vec<Expr>) -> Expr {
        Expr::Call(method, args)
    }

    fn arith(l: Expr, op: ArithOp, r: Expr) -> Expr {
        Expr::Arith(Box::new(l), op, Box::new(r))
    }

    #[test]
    fn functions_arithmetic_and_two_members_are_evaluated() {
        let int = |n| lit(Literal::Int(n));
        let s = |t: &str| lit(Literal::String(t.into()));
        let evaluated = [
            cmp(m("speed"), CompareOp::Gt, m("speed")),
            cmp(int(1), CompareOp::Eq, int(1)),
            cmp(
                arith(m("speed"), ArithOp::Add, int(1)),
                CompareOp::Gt,
                int(2),
            ),
            cmp(
                arith(m("speed"), ArithOp::DivBy, int(2)),
                CompareOp::Le,
                int(2),
            ),
            cmp(Expr::Neg(Box::new(m("speed"))), CompareOp::Lt, int(0)),
            cmp(
                call_of(Method::Round, vec![m("speed")]),
                CompareOp::Eq,
                int(3),
            ),
            cmp(
                call_of(Method::Length, vec![m("name")]),
                CompareOp::Ne,
                int(3),
            ),
            cmp(
                call_of(Method::Substring, vec![m("name"), int(1), int(2)]),
                CompareOp::Eq,
                s("ab"),
            ),
            call_of(
                Method::Contains,
                vec![call_of(Method::ToLower, vec![m("name")]), s("a")],
            ),
            call_of(Method::StartsWith, vec![m("name"), m("name")]),
            cmp(
                call_of(Method::Year, vec![call_of(Method::Now, vec![])]),
                CompareOp::Ge,
                int(2026),
            ),
            cmp(
                arith(m("speed"), ArithOp::Mul, int(2)),
                CompareOp::Ne,
                lit(Literal::Null),
            ),
            Expr::Not(Box::new(cmp(m("speed"), CompareOp::Eq, m("speed")))),
            lit(Literal::Bool(true)),
            // a Boolean expression as a value
            cmp(
                cmp(m("speed"), CompareOp::Gt, int(1)),
                CompareOp::Eq,
                lit(Literal::Bool(true)),
            ),
            cmp(
                Expr::Any {
                    collection: MemberPath {
                        variable: None,
                        path: vec!["tags".into()],
                    },
                    lambda: None,
                },
                CompareOp::Ne,
                m("on"),
            ),
            call_of(Method::MatchesPattern, vec![m("name"), s("^a+$")]),
        ];
        for e in evaluated {
            evaluated_alone(&e);
        }
    }

    #[test]
    fn literals_arithmetic_corners_and_unchecked_types_are_evaluated() {
        let int = |n| lit(Literal::Int(n));
        let s = |t: &str| lit(Literal::String(t.into()));
        let evaluated = [
            cmp(arith(int(1), ArithOp::Add, int(2)), CompareOp::Eq, int(3)),
            cmp(
                arith(m("speed"), ArithOp::Div, int(2)),
                CompareOp::Gt,
                int(1),
            ),
            cmp(
                arith(m("speed"), ArithOp::Mod, int(2)),
                CompareOp::Eq,
                int(0),
            ),
            cmp(
                int(1),
                CompareOp::Lt,
                arith(m("speed"), ArithOp::Sub, int(1)),
            ),
            cmp(
                lit(Literal::Null),
                CompareOp::Eq,
                arith(m("speed"), ArithOp::Add, int(1)),
            ),
            cmp(m("name"), CompareOp::Ne, m("name")),
            // two durations compare as durations (T-5905)
            cmp(m("span"), CompareOp::Lt, m("span")),
            // a type this surface does not check compares with any
            cmp(m("clip"), CompareOp::Eq, m("name")),
            cmp(m("name"), CompareOp::Eq, m("clip")),
            cmp(
                call_of(Method::Year, vec![lit(Literal::Date("2026-01-02".into()))]),
                CompareOp::Eq,
                int(2026),
            ),
            cmp(
                call_of(
                    Method::Hour,
                    vec![lit(Literal::DateTimeOffset("2026-01-02T03:04:05Z".into()))],
                ),
                CompareOp::Eq,
                int(3),
            ),
            cmp(
                call_of(
                    Method::Minute,
                    vec![lit(Literal::TimeOfDay("03:04:05".into()))],
                ),
                CompareOp::Eq,
                int(4),
            ),
            cmp(
                lit(Literal::Guid("01234567-89ab-cdef-0123-456789abcdef".into())),
                CompareOp::Eq,
                lit(Literal::Guid("01234567-89ab-cdef-0123-456789abcdef".into())),
            ),
            Expr::All {
                collection: MemberPath {
                    variable: None,
                    path: vec!["tags".into()],
                },
                lambda: (
                    "x".into(),
                    Box::new(call_of(
                        Method::EndsWith,
                        vec![
                            Expr::Member(MemberPath {
                                variable: Some("x".into()),
                                path: vec![],
                            }),
                            s("z"),
                        ],
                    )),
                ),
            },
        ];
        for e in evaluated {
            evaluated_alone(&e);
        }
    }

    /// `e` goes whole to the residual, and the evaluator takes it.
    fn evaluated_alone(e: &Expr) {
        let t = translate(e, &set(), &BTreeMap::new(), &[]);
        assert_eq!(
            t.as_ref().map(|t| (t.q.is_none(), t.residual.len())),
            Ok((true, 1)),
            "{e:?}"
        );
        let now = DateTime::parse("2026-10-11T12:00:00Z");
        assert!(
            t.is_ok_and(|t| now.is_some_and(|now| program(&t, now).is_ok_and(|p| p.is_some()))),
            "the evaluator takes {e:?}"
        );
    }

    #[test]
    fn an_operand_of_another_type_is_bad_and_a_duration_is_later() {
        let int = |n| lit(Literal::Int(n));
        let s = |t: &str| lit(Literal::String(t.into()));
        // URL 5.1.1: an operand of a known type the operator does not take
        let bad = [
            cmp(
                call_of(Method::Length, vec![m("speed")]),
                CompareOp::Eq,
                int(1),
            ),
            cmp(
                arith(m("name"), ArithOp::Add, int(1)),
                CompareOp::Eq,
                int(2),
            ),
            cmp(
                arith(m("speed"), ArithOp::Add, s("a")),
                CompareOp::Eq,
                int(2),
            ),
            cmp(Expr::Neg(Box::new(m("on"))), CompareOp::Eq, int(2)),
            cmp(
                call_of(Method::Year, vec![m("speed")]),
                CompareOp::Eq,
                int(2),
            ),
            cmp(m("speed"), CompareOp::Eq, m("name")),
            cmp(m("name"), CompareOp::Eq, m("speed")),
            cmp(m("on"), CompareOp::Gt, m("on")),
            call_of(Method::Length, vec![m("name")]),
            arith(m("speed"), ArithOp::Add, int(1)),
            call_of(Method::Substring, vec![m("name")]),
        ];
        for e in bad {
            assert_eq!(q(&e), Err(Refused::Bad), "{e:?}");
        }
        // a duration, a geography as a value and a JSON member's keys stay later
        let later = [
            cmp(
                call_of(Method::GeoIntersects, vec![m("location"), s("x")]),
                CompareOp::Eq,
                lit(Literal::Bool(true)),
            ),
            cmp(
                m("speed"),
                CompareOp::Gt,
                arith(lit(Literal::Null), ArithOp::Add, int(1)),
            ),
            // an ordered comparison with null is no row
            cmp(
                arith(m("speed"), ArithOp::Add, int(1)),
                CompareOp::Gt,
                lit(Literal::Null),
            ),
            // a geography as a value, a value of an unchecked type as a predicate
            cmp(
                lit(Literal::Geography(Geography {
                    srid: 4326,
                    shape: odata_url::Shape::Point(odata_url::Position(vec![
                        "0".into(),
                        "0".into(),
                    ])),
                })),
                CompareOp::Eq,
                int(1),
            ),
            call_of(Method::Round, vec![m("clip")]),
            // a value of a type this surface does not check as a predicate
            Expr::Not(Box::new(m("clip"))),
            // a lambda over a Relationship's object
            Expr::Any {
                collection: MemberPath {
                    variable: None,
                    path: vec!["owner".into()],
                },
                lambda: None,
            },
        ];
        for e in later {
            assert_eq!(q(&e), Err(Refused::Later), "{e:?}");
        }
        // a duration is no number (T-5905)
        assert_eq!(
            q(&call_of(Method::Round, vec![m("span")])),
            Err(Refused::Bad)
        );
    }

    #[test]
    fn every_method_but_geo_intersects_is_a_function_of_the_residuals_table() {
        for method in Method::all() {
            match signature(method) {
                None => assert_eq!(method, Method::GeoIntersects),
                Some((func, params, _)) => {
                    let (min, max) = method.arity();
                    assert_eq!(func.arity(), (min, max), "{method:?}");
                    assert!(params.len() <= max.max(1), "{method:?}");
                }
            }
        }
        assert_eq!(family("Edm.Int16"), "integer");
        assert_eq!(family("Edm.Single"), "number");
        assert_eq!(
            takes_arg(STRING, "Edm.Stream"),
            Ok(()),
            "an unknown type is taken"
        );
        assert_eq!(takes_arg(STRING, "Edm.Duration"), Err(Refused::Bad));
        assert_eq!(literal_edm(&Literal::Null), None);
    }

    fn path(names: &[&str]) -> Expr {
        Expr::Member(MemberPath {
            variable: None,
            path: names.iter().map(|n| (*n).to_owned()).collect(),
        })
    }

    #[test]
    fn observed_at_is_the_sub_property_of_a_member_of_the_set() {
        let at = |names: &[&str], op| {
            cmp(
                path(names),
                op,
                lit(Literal::DateTimeOffset("2026-05-01T08:30:00Z".into())),
            )
        };
        assert_eq!(
            q(&at(&["speed", OBSERVED_AT], CompareOp::Ge)),
            Ok("(t0.observedAt>=2026-05-01T08:30:00Z)".into()),
            "F21, 7.2.3.2 EXAMPLE 9"
        );
        assert_eq!(
            q(&at(&["label", OBSERVED_AT], CompareOp::Lt)),
            Ok("(t0.observedAt<2026-05-01T08:30:00Z)".into()),
            "any member of the set has one"
        );
        for refused in [
            at(&["hidden", OBSERVED_AT], CompareOp::Ge),
            at(&["speed", "@NgsiLd.datasetId"], CompareOp::Eq),
            at(&["owner", "name", OBSERVED_AT], CompareOp::Eq),
        ] {
            assert_eq!(q(&refused), Err(Refused::Later), "{refused:?}");
        }
        let by = [OrderBy {
            expr: path(&["speed", OBSERVED_AT]),
            descending: false,
        }];
        assert_eq!(
            order(&by, &set(), &BTreeMap::new(), &mut Translated::default()),
            Err(Refused::Later),
            "orderBy reads a `.` as a path: not this round's"
        );
    }

    #[test]
    fn any_over_a_list_property_is_membership() {
        let var = |name: &str, path: &[&str]| {
            Expr::Member(MemberPath {
                variable: Some(name.into()),
                path: path.iter().map(|p| (*p).to_owned()).collect(),
            })
        };
        let x = || {
            Expr::Member(MemberPath {
                variable: Some("x".into()),
                path: Vec::new(),
            })
        };
        let any = |names: &[&str], body: Expr| Expr::Any {
            collection: MemberPath {
                variable: None,
                path: names.iter().map(|n| (*n).to_owned()).collect(),
            },
            lambda: Some(("x".into(), Box::new(body))),
        };
        let a = || lit(Literal::String("a".into()));
        assert_eq!(
            q(&any(&["tags"], cmp(x(), CompareOp::Eq, a()))),
            Ok(r#"(t0=="a")"#.into()),
            "F22, 7.2.3.4"
        );
        assert_eq!(
            q(&any(&["tags"], cmp(a(), CompareOp::Eq, x()))),
            Ok(r#"(t0=="a")"#.into())
        );
        let listed = Expr::In(
            Box::new(x()),
            vec![Literal::String("a".into()), Literal::String("b".into())],
        );
        assert_eq!(q(&any(&["tags"], listed)), Ok(r#"(t0=="a","b")"#.into()));
        // F23: any other body is evaluated, its variable a `Var`
        for evaluated in [
            any(&["tags"], cmp(x(), CompareOp::Ne, a())),
            any(&["tags"], cmp(x(), CompareOp::Eq, lit(Literal::Null))),
            any(&["tags"], cmp(m("name"), CompareOp::Eq, a())),
            any(
                &["tags"],
                Expr::In(Box::new(m("name")), vec![Literal::String("a".into())]),
            ),
        ] {
            let t = translate(&evaluated, &set(), &BTreeMap::new(), &[]);
            assert_eq!(
                t.map(|t| (t.q, t.residual.len())),
                Ok((None, 1)),
                "{evaluated:?}"
            );
        }
        let scalar = any(&["name"], cmp(x(), CompareOp::Eq, a()));
        assert_eq!(q(&scalar), Err(Refused::Bad), "no list to range over");
        for refused in [
            any(&["scope"], cmp(x(), CompareOp::Eq, a())),
            any(&["hidden"], cmp(x(), CompareOp::Eq, a())),
            any(&["tags"], cmp(var("x", &["sub"]), CompareOp::Eq, a())),
            Expr::Any {
                collection: MemberPath {
                    variable: Some("y".into()),
                    path: vec!["tags".into()],
                },
                lambda: Some(("x".into(), Box::new(cmp(x(), CompareOp::Eq, a())))),
            },
        ] {
            assert_eq!(q(&refused), Err(Refused::Later), "{refused:?}");
        }
        let unknown = || Expr::Alias("missing".into());
        for bad in [
            any(&["tags"], cmp(var("y", &[]), CompareOp::Eq, a())),
            any(&["tags"], cmp(x(), CompareOp::Eq, unknown())),
            any(&["tags"], cmp(unknown(), CompareOp::Eq, x())),
        ] {
            assert_eq!(q(&bad), Err(Refused::Bad), "an unknown alias: {bad:?}");
        }
    }

    #[test]
    fn language_and_vocab_eq_are_mapped() {
        let eq = |name: &str| cmp(m(name), CompareOp::Eq, lit(Literal::String("s".into())));
        assert_eq!(
            q(&eq("label")),
            Ok(r#"(t0[@none]=="s")"#.into()),
            "F31, T-5206: the text the read serves"
        );
        let t = translate(&eq("status"), &set(), &BTreeMap::new(), &[]).unwrap_or_default();
        assert_eq!(
            (t.q.as_deref(), t.expand_values.as_slice()),
            (Some(r#"(t0=="s")"#), ["t0".to_owned()].as_slice()),
            "F32"
        );
        let twice = Expr::Or(Box::new(eq("status")), Box::new(eq("status")));
        let t = translate(&twice, &set(), &BTreeMap::new(), &[]).unwrap_or_default();
        assert_eq!(t.expand_values, ["t0"], "one term once");
        for refused in [
            cmp(m("label"), CompareOp::Ne, lit(Literal::String("s".into()))),
            cmp(m("status"), CompareOp::Gt, lit(Literal::String("s".into()))),
            cmp(m("label"), CompareOp::Eq, lit(Literal::Int(1))),
            cmp(m("status"), CompareOp::Eq, lit(Literal::Null)),
        ] {
            assert_eq!(q(&refused), Err(Refused::Later), "{refused:?}");
        }
    }

    #[test]
    fn geo_intersects_is_a_top_level_geo_query() {
        let area = |srid, ring: &[(&str, &str)]| {
            let ring = ring
                .iter()
                .map(|(x, y)| odata_url::Position(vec![(*x).into(), (*y).into()]))
                .collect();
            lit(Literal::Geography(Geography {
                srid,
                shape: odata_url::Shape::Polygon(vec![ring]),
            }))
        };
        let square = [("0", "0"), ("1", "0"), ("1", "1"), ("0", "0")];
        let within = |name: &str, area| Expr::Call(Method::GeoIntersects, vec![m(name), area]);
        let both = Expr::And(
            Box::new(within("location", area(4326, &square))),
            Box::new(cmp(m("speed"), CompareOp::Gt, lit(Literal::Int(0)))),
        );
        let t = translate(&both, &set(), &BTreeMap::new(), &[]).unwrap_or_default();
        assert_eq!(
            (t.geo, t.q.as_deref()),
            (
                Some((
                    "t0".to_owned(),
                    "[[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,0.0]]]".to_owned()
                )),
                Some("(t1>0)")
            ),
            "F29"
        );
        let refused = |e: &Expr| translate(e, &set(), &BTreeMap::new(), &[]).map(|_| ());
        let nested = Expr::Or(
            Box::new(within("location", area(4326, &square))),
            Box::new(cmp(m("speed"), CompareOp::Gt, lit(Literal::Int(0)))),
        );
        let twice = Expr::And(
            Box::new(within("location", area(4326, &square))),
            Box::new(within("location", area(4326, &square))),
        );
        // T-3758: a second area, or one below `or`, is evaluated
        for (e, pushed) in [(nested, false), (twice, true)] {
            let t = translate(&e, &set(), &BTreeMap::new(), &[]).unwrap_or_default();
            assert_eq!(t.geo.is_some(), pushed, "{e:?}");
            assert_eq!(t.residual.len(), 1, "{e:?}");
        }
        for e in [
            within("location", area(3857, &square)),
            within("hidden", area(4326, &square)),
            Expr::Call(Method::GeoIntersects, vec![m("location")]),
            Expr::Call(
                Method::GeoIntersects,
                vec![lit(Literal::Int(1)), area(4326, &square)],
            ),
        ] {
            assert_eq!(refused(&e), Err(Refused::Later), "{e:?}");
        }
        assert_eq!(
            refused(&within("speed", area(4326, &square))),
            Err(Refused::Bad),
            "not a GeoProperty"
        );
        assert_eq!(
            refused(&within(
                "location",
                area(4326, &[("0", "x"), ("1", "1"), ("0", "0")])
            )),
            Err(Refused::Bad)
        );
    }

    #[test]
    fn literals_are_q_values() {
        for (lit, want) in [
            (Literal::Bool(false), "false"),
            (Literal::Int(-MAX_EXACT), "-9007199254740992"),
            (Literal::Decimal("1e2".into()), "100.0"),
            (Literal::Date("2026-10-11".into()), "2026-10-11"),
            (
                Literal::DateTimeOffset("2026-10-11T08:00:00Z".into()),
                "2026-10-11T08:00:00Z",
            ),
        ] {
            assert_eq!(value(&lit), Ok(want.to_owned()), "{lit:?}");
        }
        // NGSI-LD stores these as text q does not order: the residual compares them (T-5905)
        for lit in [
            Literal::TimeOfDay("08:00:00".into()),
            Literal::Duration("P1D".into()),
            Literal::Binary("AQID".into()),
            Literal::Decimal("INF".into()),
            Literal::Decimal("NaN".into()),
        ] {
            assert_eq!(value(&lit), Err(Refused::Later), "{lit:?}");
        }
    }

    #[test]
    fn orderby_names_id_and_scalar_terms() {
        let by = |name: &str, descending| OrderBy {
            expr: m(name),
            descending,
        };
        let mut t = translate(
            &cmp(m("name"), CompareOp::Eq, lit(Literal::String("1".into()))),
            &set(),
            &BTreeMap::new(),
            &[],
        )
        .unwrap_or_default();
        assert_eq!(
            order(
                &[by("speed", true), by("id", false), by("name", false)],
                &set(),
                &BTreeMap::new(),
                &mut t
            ),
            Ok(())
        );
        assert_eq!(
            t.order.as_deref(),
            Some("t1;desc,id,t0"),
            "one IRI, one term, shared with q"
        );
        let mut t = Translated::default();
        for (items, refused) in [
            (vec![by("tags", false)], Refused::Bad),
            (vec![by("doc", false)], Refused::Bad),
            (vec![by("owner", false)], Refused::Later),
            (vec![by("scope", false)], Refused::Later),
            (
                vec![OrderBy {
                    expr: lit(Literal::Int(1)),
                    descending: false,
                }],
                Refused::Later,
            ),
            (
                vec![OrderBy {
                    expr: Expr::Member(MemberPath {
                        variable: None,
                        path: vec!["speed".into(), "x".into()],
                    }),
                    descending: false,
                }],
                Refused::Later,
            ),
        ] {
            assert_eq!(
                order(&items, &set(), &BTreeMap::new(), &mut t),
                Err(refused),
                "{items:?}"
            );
        }
    }

    #[test]
    fn a_translation_past_the_broker_bound_is_too_complex() {
        let long = "x".repeat(MAX_Q_BYTES);
        let e = cmp(m("name"), CompareOp::Eq, lit(Literal::String(long)));
        assert_eq!(
            translate(&e, &set(), &BTreeMap::new(), &[]),
            Err(Refused::TooComplex)
        );
    }

    /// The answer of a residual filter for an Entity whose `t0` holds
    /// `value` (`None`: no such Attribute), and whether it was pushed.
    fn evaluated(e: &Expr, value: Option<serde_json::Value>) -> Option<residual::Truth> {
        let t = translate(e, &set(), &BTreeMap::new(), &[]).ok()?;
        let now = DateTime::parse("2026-10-11T12:00:00Z")?;
        let program = program(&t, now).ok()??;
        let mut entity = serde_json::json!({"id": "urn:ngsi-ld:Car:1", "type": "Car"});
        if let (Some(v), Some(members)) = (value, entity.as_object_mut()) {
            members.insert(
                "t0".into(),
                serde_json::json!({"type": "Property", "value": v}),
            );
        }
        program.eval(&entity).ok()
    }

    #[test]
    fn not_is_evaluated_with_odatas_null_rules() {
        use residual::Truth::{False, True, Unknown};
        use serde_json::json;
        let not = |e: Expr| Expr::Not(Box::new(e));
        // F11, URL 5.1.1.1.3: a missing value, or one of another type,
        // compares false, so `not` of the comparison holds there
        let gt = not(cmp(m("speed"), CompareOp::Gt, lit(Literal::Int(5))));
        for (value, want) in [
            (Some(json!(3)), True),
            (Some(json!(7)), False),
            (Some(json!(5.5)), False),
            (Some(json!("x")), True),
            (None, True),
        ] {
            assert_eq!(evaluated(&gt, value.clone()), Some(want), "{value:?}");
        }
        // URL 5.1.1.1.2: null is not equal to any value
        let eq = not(cmp(
            m("name"),
            CompareOp::Eq,
            lit(Literal::String("a".into())),
        ));
        assert_eq!(evaluated(&eq, None), Some(True));
        assert_eq!(evaluated(&eq, Some(json!("a"))), Some(False));
        let listed = not(Expr::In(
            Box::new(m("speed")),
            vec![Literal::Int(1), Literal::Int(2)],
        ));
        assert_eq!(evaluated(&listed, None), Some(True));
        assert_eq!(evaluated(&listed, Some(json!(2))), Some(False));
        // a double negation is the comparison again
        let ne = not(not(cmp(m("speed"), CompareOp::Ne, lit(Literal::Int(5)))));
        assert_eq!(evaluated(&ne, None), Some(True));
        assert_eq!(evaluated(&ne, Some(json!(5))), Some(False));
        // a string function or a Boolean member over nothing stays null
        let flag = not(m("on"));
        assert_eq!(evaluated(&flag, None), Some(Unknown));
        assert_eq!(evaluated(&flag, Some(json!(false))), Some(True));
        let starts = not(not(Expr::Call(
            Method::StartsWith,
            vec![m("name"), lit(Literal::String("Sta".into()))],
        )));
        assert_eq!(evaluated(&starts, None), Some(Unknown));
        assert_eq!(evaluated(&starts, Some(json!("Station"))), Some(True));
        // `not (p eq null)` is F10, pushed
        let present = not(cmp(m("speed"), CompareOp::Eq, lit(Literal::Null)));
        assert_eq!(evaluated(&present, None), None);
        assert_eq!(q(&present), Ok("(t0)".into()));
    }

    #[test]
    fn a_conjunct_goes_whole_to_the_residual_and_the_rest_is_pushed() {
        let e = Expr::And(
            Box::new(cmp(m("speed"), CompareOp::Gt, lit(Literal::Int(1)))),
            Box::new(Expr::Not(Box::new(Expr::Or(
                Box::new(cmp(
                    m("name"),
                    CompareOp::Eq,
                    lit(Literal::String("a".into())),
                )),
                Box::new(cmp(m("speed"), CompareOp::Lt, lit(Literal::Int(9)))),
            )))),
        );
        let t = translate(&e, &set(), &BTreeMap::new(), &[]).unwrap_or_default();
        assert_eq!(t.q.as_deref(), Some("(t0>1)"));
        assert_eq!(t.residual.len(), 1);
        assert_eq!(t.linked, 0, "a failed push leaves no join behind");
        // an offset date-time is one instant `q` cannot say: evaluated
        let offset = cmp(
            Expr::Member(MemberPath {
                variable: None,
                path: vec!["speed".into(), OBSERVED_AT.into()],
            }),
            CompareOp::Lt,
            lit(Literal::DateTimeOffset("2026-01-01T00:00:00+01:00".into())),
        );
        let t = translate(&offset, &set(), &BTreeMap::new(), &[]).unwrap_or_default();
        assert_eq!((t.q, t.residual.len()), (None, 1));
        // a `not` over what is still a later round's stays 501
        let later = Expr::Not(Box::new(cmp(
            m("tags"),
            CompareOp::Eq,
            lit(Literal::String("a".into())),
        )));
        assert_eq!(q(&later), Err(Refused::Later));
        // past the evaluator's bounds: TooComplex (ADR-0028 rule 7)
        let mut deep = cmp(m("speed"), CompareOp::Gt, lit(Literal::Int(1)));
        for _ in 0..80 {
            deep = Expr::Not(Box::new(deep));
        }
        let t = translate(&deep, &set(), &BTreeMap::new(), &[]).unwrap_or_default();
        let now = DateTime::parse("2026-10-11T12:00:00Z");
        assert_eq!(
            now.map(|now| program(&t, now).map(|_| ())),
            Some(Err(Refused::TooComplex))
        );
    }

    #[test]
    fn a_literal_of_another_type_than_its_member_is_bad() {
        // T-3725: URL 5.1.1.1, and 5.1.1.5 for an integer past its range.
        let int = |n| Literal::Int(n);
        let dec = |raw: &str| Literal::Decimal(raw.into());
        let string = || Literal::String("1".into());
        let guid = || Literal::Guid("01234567-89ab-cdef-0123-456789abcdef".into());
        for (edm, lit) in [
            ("Edm.String", string()),
            ("Edm.Double", int(1)),
            ("Edm.Double", dec("1.5")),
            ("Edm.Single", dec("NaN")),
            ("Edm.Decimal", dec("99999999999999999999")),
            ("Edm.Int64", int(i64::MAX)),
            ("Edm.Int64", dec("1.5")),
            ("Edm.Int16", int(-32768)),
            ("Edm.Byte", int(255)),
            ("Edm.SByte", int(-128)),
            ("Edm.Int32", int(i64::from(i32::MAX))),
            ("Edm.Guid", guid()),
            ("Edm.Boolean", Literal::Bool(true)),
            ("Edm.Date", Literal::Date("2026-01-01".into())),
            ("Edm.TimeOfDay", Literal::TimeOfDay("09:30:00".into())),
            (
                "Edm.DateTimeOffset",
                Literal::DateTimeOffset("2026-01-01T00:00:00Z".into()),
            ),
            ("Edm.String", Literal::Null),
            ("Edm.Untyped", int(1)),
        ] {
            assert!(fits(edm, &lit), "{edm} {lit:?}");
        }
        for (edm, lit) in [
            ("Edm.String", int(1)),
            ("Edm.String", guid()),
            ("Edm.Double", string()),
            ("Edm.Int64", string()),
            ("Edm.Int64", dec("9223372036854775808")),
            ("Edm.Int64", dec("-9223372036854775809")),
            ("Edm.Int16", int(32768)),
            ("Edm.Byte", int(-1)),
            ("Edm.SByte", int(128)),
            ("Edm.Int32", int(i64::from(i32::MAX) + 1)),
            ("Edm.Guid", string()),
            ("Edm.Boolean", int(1)),
            ("Edm.Date", string()),
            ("Edm.DateTimeOffset", Literal::Date("2026-01-01".into())),
        ] {
            assert!(!fits(edm, &lit), "{edm} {lit:?}");
        }
        for e in [
            cmp(m("name"), CompareOp::Gt, lit(int(10))),
            cmp(lit(string()), CompareOp::Eq, m("speed")),
            Expr::In(Box::new(m("name")), vec![int(1), int(2)]),
            Expr::In(Box::new(m("speed")), vec![string()]),
        ] {
            assert_eq!(q(&e), Err(Refused::Bad), "{e:?}");
        }
    }

    fn ordered(set: &Set, names: &[(&str, bool)]) -> Result<Translated, Refused> {
        let items: Vec<OrderBy> = names
            .iter()
            .map(|(name, descending)| OrderBy {
                expr: m(name),
                descending: *descending,
            })
            .collect();
        let mut t = Translated::default();
        order(&items, set, &BTreeMap::new(), &mut t).map(|()| t)
    }

    #[test]
    fn a_system_member_is_sorted_by_the_surface_unless_the_model_names_it() -> Result<(), Refused> {
        // Q9: the surface sorts, reading the value keys beside it
        let t = ordered(
            &set(),
            &[("createdAt", false), ("speed", true), ("id", false)],
        )?;
        assert_eq!(t.order, None);
        assert!(t.sort_system);
        assert_eq!(t.sort_reads, ["t0"]);
        assert_eq!(
            t.sort.map(|keys| keys
                .into_iter()
                .map(|k| (k.by, k.descending))
                .collect::<Vec<_>>()),
            Some(vec![
                (SortBy::System("createdAt".into()), false),
                (SortBy::Value("t0".into()), true),
                (SortBy::Id, false),
            ])
        );
        // without one, `orderBy` is pushed
        let t = ordered(&set(), &[("speed", true), ("id", false)])?;
        assert_eq!((t.order.as_deref(), t.sort), (Some("t0;desc,id"), None));
        // a model member of that name is an Attribute like any other
        let mut named = set();
        named.members.insert(
            "modifiedAt".into(),
            Member {
                iri: "https://e.org/modifiedAt".into(),
                shape: Shape::Value {
                    edm: "Edm.DateTimeOffset",
                    collection: false,
                },
                list: false,
                json: false,
                untyped: false,
                written: crate::edm::Written::Always,
            },
        );
        let t = ordered(&named, &[("modifiedAt", false)])?;
        assert_eq!((t.order.as_deref(), t.sort), (Some("t0"), None));
        Ok(())
    }

    #[test]
    fn every_residual_form_is_evaluated_or_refused() {
        use residual::Truth::{False, True};
        use serde_json::json;
        let not = |e: Expr| Expr::Not(Box::new(e));
        let int = |n| lit(Literal::Int(n));
        let speed = |op, n| cmp(m("speed"), op, int(n));
        let and = not(Expr::And(
            Box::new(speed(CompareOp::Gt, 1)),
            Box::new(speed(CompareOp::Lt, 9)),
        ));
        assert_eq!(evaluated(&and, Some(json!(5))), Some(False));
        assert_eq!(evaluated(&and, Some(json!(10))), Some(True));
        // a literal on the left is mirrored
        let left = not(cmp(int(5), CompareOp::Lt, m("speed")));
        assert_eq!(evaluated(&left, Some(json!(6))), Some(False));
        assert_eq!(evaluated(&left, Some(json!(5))), Some(True));
        for (op, value, want) in [
            (CompareOp::Ge, 5, False),
            (CompareOp::Ge, 4, True),
            (CompareOp::Le, 5, False),
            (CompareOp::Le, 6, True),
        ] {
            assert_eq!(
                evaluated(&not(speed(op, 5)), Some(json!(value))),
                Some(want)
            );
        }
        // `eq null` and `ne null` under `or`
        let null = |op| cmp(m("speed"), op, lit(Literal::Null));
        let absent = not(Expr::Or(
            Box::new(null(CompareOp::Eq)),
            Box::new(speed(CompareOp::Gt, 3)),
        ));
        assert_eq!(evaluated(&absent, None), Some(False));
        assert_eq!(evaluated(&absent, Some(json!(2))), Some(True));
        let present = not(Expr::Or(
            Box::new(null(CompareOp::Ne)),
            Box::new(speed(CompareOp::Gt, 3)),
        ));
        assert_eq!(evaluated(&present, None), Some(True));
        assert_eq!(evaluated(&present, Some(json!(2))), Some(False));
        let ends = not(not(Expr::Call(
            Method::EndsWith,
            vec![m("name"), lit(Literal::String("ion".into()))],
        )));
        assert_eq!(evaluated(&ends, Some(json!("Station"))), Some(True));
        let on = not(cmp(m("on"), CompareOp::Eq, lit(Literal::Bool(true))));
        assert_eq!(evaluated(&on, Some(json!(false))), Some(True));
        let half = not(cmp(
            m("speed"),
            CompareOp::Gt,
            lit(Literal::Decimal("1.5".into())),
        ));
        assert_eq!(evaluated(&half, Some(json!(1))), Some(True));
        // two residual conjuncts are one program
        let both = Expr::And(
            Box::new(not(speed(CompareOp::Gt, 8))),
            Box::new(not(speed(CompareOp::Lt, 2))),
        );
        assert_eq!(evaluated(&both, Some(json!(5))), Some(True));
        assert_eq!(evaluated(&both, Some(json!(9))), Some(False));
        // what no member type takes is Bad
        for e in [
            not(not(Expr::Call(
                Method::Contains,
                vec![m("speed"), lit(Literal::String("1".into()))],
            ))),
            not(m("speed")),
            not(cmp(m("doc"), CompareOp::Eq, int(1))),
        ] {
            assert_eq!(q(&e), Err(Refused::Bad), "{e:?}");
        }
    }

    /// T-5333: an Untyped Property compares by its JSON value, in `q` and in the residual,
    /// as NGSI-LD 4.9 does: a value of another type matches only `ne`.
    #[test]
    fn an_untyped_property_compares_by_its_json_value() {
        use residual::Truth::{False, True};
        use serde_json::json;
        let reading = |op, r: Literal| cmp(m("reading"), op, lit(r));
        for (e, want) in [
            (reading(CompareOp::Ge, Literal::Int(8372)), "(t0>=8372)"),
            (
                reading(CompareOp::Lt, Literal::Decimal("0.5".into())),
                "(t0<0.5)",
            ),
            (
                reading(CompareOp::Eq, Literal::String("not measured".into())),
                r#"(t0=="not measured")"#,
            ),
            (reading(CompareOp::Ne, Literal::Int(1)), "(!t0|t0!=1)"),
            (reading(CompareOp::Eq, Literal::Null), "(!t0)"),
        ] {
            assert_eq!(q(&e).as_deref(), Ok(want), "{e:?}");
        }
        // a type this surface does not check takes any literal
        assert_eq!(
            q(&cmp(m("clip"), CompareOp::Eq, lit(Literal::Int(1)))).as_deref(),
            Ok("(t0==1)")
        );
        // a duration takes a duration literal, which the residual compares (T-5905)
        assert_eq!(
            q(&cmp(m("span"), CompareOp::Eq, lit(Literal::Int(1)))),
            Err(Refused::Bad)
        );
        assert_eq!(
            q(&cmp(
                m("span"),
                CompareOp::Eq,
                lit(Literal::Duration("P1D".into()))
            ))
            .as_deref(),
            Ok("")
        );
        let below = Expr::Not(Box::new(reading(CompareOp::Ge, Literal::Int(5))));
        assert_eq!(evaluated(&below, Some(json!(3))), Some(True));
        assert_eq!(evaluated(&below, Some(json!(9))), Some(False));
        assert_eq!(evaluated(&below, Some(json!("not measured"))), Some(True));
        // a literal no JSON value is, and a member no filter compares
        for e in [
            reading(CompareOp::Eq, Literal::Date("2026-10-22".into())),
            reading(
                CompareOp::Eq,
                Literal::Guid("00000000-0000-0000-0000-000000000000".into()),
            ),
            cmp(m("doc"), CompareOp::Eq, lit(Literal::Int(1))),
        ] {
            assert_eq!(q(&e), Err(Refused::Bad), "{e:?}");
        }
    }

    #[test]
    fn a_date_and_a_time_of_day_are_residual_values() {
        use residual::Truth::{False, True};
        use serde_json::json;
        let mut timed = set();
        for (name, edm) in [("day", "Edm.Date"), ("at", "Edm.TimeOfDay")] {
            timed.members.insert(
                name.into(),
                Member {
                    iri: format!("https://e.org/{name}"),
                    shape: Shape::Value {
                        edm,
                        collection: false,
                    },
                    list: false,
                    json: false,
                    untyped: false,
                    written: crate::edm::Written::Always,
                },
            );
        }
        let not = |e: Expr| Expr::Not(Box::new(e));
        let now = DateTime::parse("2026-10-11T12:00:00Z");
        for (e, value, want) in [
            (
                not(cmp(
                    m("day"),
                    CompareOp::Lt,
                    lit(Literal::Date("2026-01-02".into())),
                )),
                json!({"@type": "Date", "@value": "2026-01-01"}),
                False,
            ),
            (
                not(cmp(
                    m("at"),
                    CompareOp::Lt,
                    lit(Literal::TimeOfDay("09:30:00".into())),
                )),
                json!({"@type": "Time", "@value": "10:00:00"}),
                True,
            ),
        ] {
            let t = translate(&e, &timed, &BTreeMap::new(), &[]);
            let program = t
                .and_then(|t| now.map_or(Err(Refused::Later), |now| program(&t, now)))
                .ok()
                .flatten();
            let entity = json!({"id": "urn:ngsi-ld:Car:1", "type": "Car",
                "t0": {"type": "Property", "value": value}});
            assert_eq!(
                program.and_then(|p| p.eval(&entity).ok()),
                Some(want),
                "{e:?}"
            );
        }
        // a literal the type cannot hold is Bad
        let bad = not(cmp(
            m("day"),
            CompareOp::Lt,
            lit(Literal::Date("2026-02-30".into())),
        ));
        assert_eq!(
            translate(&bad, &timed, &BTreeMap::new(), &[]).map(|_| ()),
            Err(Refused::Bad)
        );
    }

    /// The test set with a member of each Edm type T-5905 serves.
    fn typed_set() -> Set {
        let mut typed = set();
        for (name, edm) in [
            ("weight", "Edm.Single"),
            ("price", "Edm.Double"),
            ("rating", "Edm.Byte"),
            ("qty", "Edm.Int16"),
            ("ship", "Edm.Duration"),
            ("data", "Edm.Binary"),
            ("opens", "Edm.TimeOfDay"),
        ] {
            typed.members.insert(
                name.into(),
                Member {
                    iri: format!("https://e.org/{name}"),
                    shape: Shape::Value {
                        edm,
                        collection: false,
                    },
                    list: false,
                    json: false,
                    untyped: false,
                    written: crate::edm::Written::Always,
                },
            );
        }
        typed
    }

    /// T-5905: a literal past its type's range or of another type is 400 before any read, and
    /// the literals read as their residual values.
    #[test]
    fn an_edm_literal_past_its_range_or_type_is_bad() {
        let typed = typed_set();
        let c = |name: &str, op, l: Literal| cmp(m(name), op, lit(l));
        let dec = |t: &str| Literal::Decimal(t.into());
        // past the type's range, or of another type: 400 before any read
        for e in [
            c("weight", CompareOp::Eq, dec("3.5e38")),
            c("weight", CompareOp::Eq, Literal::String("3.5".into())),
            c("rating", CompareOp::Eq, Literal::Int(256)),
            c("qty", CompareOp::Gt, Literal::Int(32768)),
            c("ship", CompareOp::Eq, Literal::String("P1D".into())),
            c("ship", CompareOp::Eq, Literal::Int(86400)),
            c("data", CompareOp::Eq, Literal::String("dGVzdA==".into())),
            c("opens", CompareOp::Eq, Literal::Duration("PT1H".into())),
        ] {
            assert_eq!(
                translate(&e, &typed, &BTreeMap::new(), &[]).map(|_| ()),
                Err(Refused::Bad),
                "{e:?}"
            );
        }
        // an integer is a Single, and the literals read as their residual values
        assert!(translate(
            &c("weight", CompareOp::Gt, Literal::Int(1)),
            &typed,
            &BTreeMap::new(),
            &[]
        )
        .is_ok());
        assert_eq!(
            literal_edm(&Literal::Duration("P1D".into())),
            Some("Edm.Duration")
        );
        assert_eq!(
            literal_edm(&Literal::Binary("AQID".into())),
            Some("Edm.Binary")
        );
        assert_eq!(residual_lit(&dec("-INF")), Ok(Lit::Num(f64::NEG_INFINITY)));
        assert_eq!(format!("{:?}", residual_lit(&dec("NaN"))), "Ok(Num(NaN))");
        assert_eq!(residual_lit(&dec("1e999")), Err(Refused::Bad));
        assert_eq!(value(&dec("1e999")), Err(Refused::Bad));
        // a Single within its range is pushed as a q number
        assert_eq!(
            translate(
                &c("weight", CompareOp::Gt, dec("1.5e2")),
                &typed,
                &BTreeMap::new(),
                &[]
            )
            .map(|t| t.q),
            Ok(Some("(t0>150.0)".into()))
        );
    }

    /// T-5905: each Edm type NGSI-LD stores as text or as a JSON number is compared as its
    /// value in the residual: its literal is read, a literal past its range or of another type
    /// is 400 before any read, and the filter keeps the Entities whose value holds it.
    #[test]
    fn every_edm_type_ngsi_ld_carries_is_filtered_by_its_value() {
        use residual::Truth::{self, False, True};
        use serde_json::{json, Value as J};
        let typed = typed_set();
        let now = DateTime::parse("2026-10-28T12:00:00Z");
        let on = |e: &Expr, stored: J| -> Result<Truth, Refused> {
            let t = translate(e, &typed, &BTreeMap::new(), &[])?;
            assert_eq!(t.q, None, "{e:?} is the residual's");
            let p = program(&t, now.ok_or(Refused::Later)?)?.ok_or(Refused::Later)?;
            let entity = json!({"id": "urn:ngsi-ld:P:1", "type": "P",
                "t0": {"type": "Property", "value": stored}});
            p.eval(&entity).map_err(|_| Refused::Later)
        };
        let c = |name: &str, op, l: Literal| cmp(m(name), op, lit(l));
        let dec = |t: &str| Literal::Decimal(t.into());
        for (e, stored, want) in [
            (c("weight", CompareOp::Ne, dec("INF")), json!(3.5), True),
            (c("weight", CompareOp::Lt, dec("INF")), json!(3.5), True),
            (c("price", CompareOp::Gt, dec("-INF")), json!(0.0), True),
            (c("price", CompareOp::Eq, dec("NaN")), json!(15.5), False),
            (
                c("ship", CompareOp::Eq, Literal::Duration("PT95400S".into())),
                json!("P1DT2H30M"),
                True,
            ),
            (
                c("ship", CompareOp::Gt, Literal::Duration("PT1H".into())),
                json!("PT45S"),
                False,
            ),
            (
                c("ship", CompareOp::Eq, Literal::Duration("-P1D".into())),
                json!("-PT24H"),
                True,
            ),
            (
                c("data", CompareOp::Eq, Literal::Binary("dGVzdA".into())),
                json!("dGVzdA=="),
                True,
            ),
            (
                c("data", CompareOp::Eq, Literal::Binary(String::new())),
                json!(""),
                True,
            ),
            (
                c("data", CompareOp::Ne, Literal::Binary("AQID".into())),
                json!("dGVzdA=="),
                True,
            ),
            (
                c(
                    "opens",
                    CompareOp::Eq,
                    Literal::TimeOfDay("09:30:00".into()),
                ),
                json!("09:30:00"),
                True,
            ),
            (
                c(
                    "opens",
                    CompareOp::Eq,
                    Literal::TimeOfDay("09:30:00".into()),
                ),
                json!("09:30:00Z"),
                True,
            ),
            (
                c(
                    "opens",
                    CompareOp::Ge,
                    Literal::TimeOfDay("09:30:00".into()),
                ),
                json!("09:29:59.5Z"),
                False,
            ),
        ] {
            assert_eq!(on(&e, stored), Ok(want), "{e:?}");
        }
    }

    /// `$search` folds to a constant where no String property can hold a term, and reads only
    /// the single-valued String properties otherwise (T-3750).
    #[test]
    fn a_search_reads_the_string_properties_and_folds_without_them() {
        let word = |w: &str| Box::new(Search::Word(w.into()));
        let mut bare = set();
        bare.members.retain(|name, _| name != "name");
        let always = |s: &Search| searched(s, &bare);
        assert_eq!(
            always(&Search::Word("x".into())),
            Some(lit(Literal::Bool(false)))
        );
        assert_eq!(always(&Search::Not(word("x"))), None);
        assert_eq!(
            always(&Search::Or(word("x"), Box::new(Search::Not(word("y"))))),
            None
        );
        assert_eq!(
            always(&Search::And(word("x"), Box::new(Search::Not(word("y"))))),
            Some(lit(Literal::Bool(false)))
        );
        let not = |w: &str| Box::new(Search::Not(word(w)));
        assert_eq!(always(&Search::And(not("x"), not("y"))), None);
        assert_eq!(
            always(&Search::And(not("x"), word("y"))),
            Some(lit(Literal::Bool(false)))
        );
        assert_eq!(always(&Search::Or(not("x"), word("y"))), None);
        // `name` is the one single-valued String: `tags` is a collection, `doc` no String
        let one = searched(&Search::Phrase("A b".into()), &set());
        let name = || m("name");
        let want = Expr::And(
            Box::new(cmp(name(), CompareOp::Ne, lit(Literal::Null))),
            Box::new(Expr::Call(
                Method::Contains,
                vec![
                    Expr::Call(Method::ToLower, vec![name()]),
                    lit(Literal::String("a b".into())),
                ],
            )),
        );
        assert_eq!(one, Some(want.clone()));
        let both = Search::And(word("x"), Box::new(Search::Or(word("y"), word("z"))));
        assert!(matches!(
            searched(&both, &set()),
            Some(Expr::And(_, r)) if matches!(*r, Expr::Or(..))
        ));
        let negated = searched(&Search::Not(Box::new(Search::Phrase("A b".into()))), &set());
        assert_eq!(negated, Some(Expr::Not(Box::new(want))));
        let translated = translate(
            &searched(&Search::Word("x".into()), &set()).unwrap_or(lit(Literal::Null)),
            &set(),
            &BTreeMap::new(),
            &[],
        );
        // the presence is pushed as `q`, the case-insensitive match is the residual's
        assert_eq!(
            translated.ok().map(|t| (t.q.is_some(), t.residual.len())),
            Some((true, 1))
        );
    }

    /// T-5906: a Car whose `parts` (collection) and `maker` (single) navigate to Parts, a Part
    /// with `lang` and `parts` of its own.
    fn navigating() -> (Set, BTreeMap<String, Set>) {
        let mut car = set();
        let nav = |iri: &str, collection| Member {
            iri: iri.into(),
            shape: Shape::Nav {
                target: "Parts".into(),
                collection,
            },
            list: false,
            json: false,
            untyped: false,
            written: crate::edm::Written::Always,
        };
        car.members
            .insert("parts".into(), nav("https://e.org/parts", true));
        car.members
            .insert("maker".into(), nav("https://e.org/maker", false));
        let mut part = set();
        part.type_iri = "https://e.org/Part".into();
        part.members
            .insert("parts".into(), nav("https://e.org/parts", true));
        (car, BTreeMap::from([("Parts".to_owned(), part)]))
    }

    fn mp(variable: Option<&str>, names: &[&str]) -> MemberPath {
        MemberPath {
            variable: variable.map(str::to_owned),
            path: names.iter().map(|n| (*n).to_owned()).collect(),
        }
    }

    fn over(variable: &str, names: &[&str]) -> Expr {
        Expr::Member(mp(Some(variable), names))
    }

    fn any(collection: MemberPath, lambda: Option<(&str, Expr)>) -> Expr {
        Expr::Any {
            collection,
            lambda: lambda.map(|(v, body)| (v.to_owned(), Box::new(body))),
        }
    }

    #[test]
    fn any_over_a_navigation_is_a_linked_entity_term() {
        let (car, sets) = navigating();
        let q =
            |e: &Expr| translate(e, &car, &sets, &[]).map(|t| (t.q.unwrap_or_default(), t.linked));
        let en = |v: &str| {
            cmp(
                over(v, &["name"]),
                CompareOp::Eq,
                lit(Literal::String("EN".into())),
            )
        };
        // URL 5.1.1.13.1 over a Relationship: one target the caller reads matches (4.9)
        assert_eq!(
            q(&any(mp(None, &["parts"]), Some(("d", en("d"))))),
            Ok((r#"(t0{t1:t2}=="EN")"#.to_owned(), 1))
        );
        // `or` distributes over the targets, `contains` is a pattern on the target's term
        let or = Expr::Or(Box::new(en("d")), Box::new(en("d")));
        assert_eq!(
            q(&any(mp(None, &["parts"]), Some(("d", or)))),
            Ok((r#"((t0{t1:t2}=="EN")|(t0{t1:t2}=="EN"))"#.to_owned(), 1))
        );
        let contains = Expr::Call(
            Method::Contains,
            vec![over("d", &["name"]), lit(Literal::String("a.b".into()))],
        );
        assert_eq!(
            q(&any(mp(None, &["parts"]), Some(("d", contains)))),
            Ok((r#"(t0{t1:t2}~="a\.b")"#.to_owned(), 1))
        );
        // `any()` is the Relationship being there
        assert_eq!(
            q(&any(mp(None, &["parts"]), None)),
            Ok(("(t0)".to_owned(), 0))
        );
        // two hops nest the terms and the join
        let nested = any(
            mp(None, &["parts"]),
            Some(("r", any(mp(Some("r"), &["parts"]), Some(("d", en("d")))))),
        );
        assert_eq!(
            q(&nested),
            Ok((r#"(t0{t1:t0{t1:t2}}=="EN")"#.to_owned(), 2))
        );
        let nested_empty = any(
            mp(None, &["parts"]),
            Some(("r", any(mp(Some("r"), &["parts"]), None))),
        );
        assert_eq!(q(&nested_empty), Ok(("(t0{t1:t0})".to_owned(), 1)));
    }

    #[test]
    fn what_any_over_a_navigation_cannot_say_in_q_is_refused() {
        let (car, sets) = navigating();
        let q = |e: &Expr| translate(e, &car, &sets, &[]).map(|t| t.q);
        let en = cmp(
            over("d", &["name"]),
            CompareOp::Eq,
            lit(Literal::String("EN".into())),
        );
        // `and` may hold on two targets, `all` has no q form: neither is pushed
        let and = Expr::And(Box::new(en.clone()), Box::new(en.clone()));
        assert_eq!(
            q(&any(mp(None, &["parts"]), Some(("d", and)))),
            Err(Refused::Later)
        );
        let all = Expr::All {
            collection: mp(None, &["parts"]),
            lambda: ("d".into(), Box::new(en.clone())),
        };
        assert_eq!(q(&all), Err(Refused::Later));
        // a member the target does not have, a path below it, a variable out of scope
        let missing = cmp(over("d", &["nope"]), CompareOp::Eq, lit(Literal::Int(1)));
        assert_eq!(
            q(&any(mp(None, &["parts"]), Some(("d", missing)))),
            Err(Refused::Later)
        );
        let deep = cmp(
            over("d", &["name", "x"]),
            CompareOp::Eq,
            lit(Literal::Int(1)),
        );
        assert_eq!(
            q(&any(mp(None, &["parts"]), Some(("d", deep)))),
            Err(Refused::Later)
        );
        let stray = cmp(over("x", &["name"]), CompareOp::Eq, lit(Literal::Int(1)));
        assert_eq!(
            q(&any(mp(None, &["parts"]), Some(("d", stray)))),
            Err(Refused::Later)
        );
        // a single-valued navigation is no collection to range over
        assert_eq!(q(&any(mp(None, &["maker"]), None)), Err(Refused::Later));
        // a path through a single navigation to a collection is no lambda this round maps
        assert_eq!(
            q(&any(mp(None, &["maker", "parts"]), None)),
            Err(Refused::Later)
        );
        // a value of another type than the target's member is 400, as on the set itself
        let typed = cmp(over("d", &["name"]), CompareOp::Eq, lit(Literal::Int(1)));
        assert_eq!(
            q(&any(mp(None, &["parts"]), Some(("d", typed)))),
            Err(Refused::Bad)
        );
        let doc = cmp(over("d", &["doc"]), CompareOp::Eq, lit(Literal::Int(1)));
        assert_eq!(
            q(&any(mp(None, &["parts"]), Some(("d", doc)))),
            Err(Refused::Bad)
        );
    }

    #[test]
    fn orderby_a_member_of_a_single_navigation_target_is_a_surface_sort() {
        let (car, sets) = navigating();
        let by = |names: &[&str], descending| OrderBy {
            expr: path(names),
            descending,
        };
        let mut t = Translated::default();
        assert_eq!(
            order(
                &[by(&["maker", "name"], true), by(&["name"], false)],
                &car,
                &sets,
                &mut t
            ),
            Ok(())
        );
        let keys: Vec<(SortBy, bool)> = t
            .sort
            .unwrap_or_default()
            .into_iter()
            .map(|k| (k.by, k.descending))
            .collect();
        assert_eq!(
            keys,
            vec![
                (SortBy::Linked("t0".into(), "t1".into()), true),
                (SortBy::Value("t1".into()), false)
            ]
        );
        assert_eq!(
            (t.linked, t.sort_reads),
            (1, vec!["t0".to_owned(), "t1".to_owned()])
        );
        // a collection, a member the target lacks or does not sort, a deeper path
        for names in [
            &["parts", "name"][..],
            &["maker", "nope"],
            &["maker", "doc"],
            &["maker", "name", "x"],
        ] {
            let refused = order(&[by(names, false)], &car, &sets, &mut Translated::default());
            assert!(refused.is_err(), "{names:?}");
        }
        assert_eq!(
            order(
                &[by(&["maker", "doc"], false)],
                &car,
                &sets,
                &mut Translated::default()
            ),
            Err(Refused::Bad)
        );
    }
}
