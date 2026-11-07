// SPDX-License-Identifier: EUPL-1.2
//! `$apply` (OData Data Aggregation 4.0 section 3, Q19, T-3937, T-3985): `identity`, `filter`,
//! `search`, `orderby`, `top`, `skip`, `compute`, `topcount` to `bottompercent`, `concat`,
//! `aggregate` and `groupby` over the whole answer of the view, read through one bounded scan:
//! past `RESIDUAL_SCAN` Entities the read is refused, never aggregated in part (ADR-0028). Each
//! expression is a residual value of every instance, as a `$compute` item is: a member the view
//! hides is an unknown name (400), one the engine hides reads as absent, and an Entity either
//! hides is not in the answer, so nothing hidden is counted, ranked or summed. After `aggregate`
//! or `groupby` the rows are no Entities: a later transformation reads each as an Entity whose
//! Properties are the row's names.
//!
//! Decided (worker-5, owner may override): `sum`, `average`, `min` and `max` fold the non-null
//! values and are `null` without one, `countdistinct` counts the distinct non-null values,
//! `$count` the instances (Data Aggregation 4.0 section 3.1.3); `sum` and `average` take
//! numbers only (400 otherwise). `topcount` to `bottompercent` rank the instances with a value,
//! highest or lowest first, and keep them in that order; `topsum` and `bottomsum` keep the
//! fewest whose values reach the first parameter, the percentages the fewest that reach that
//! share of the sum of every value. A `compute` name stays on the Entities and is written with
//! them, but not past a `concat`; `search` after an aggregation is 501. Beside `$apply`,
//! `$filter`, `$orderby`, `$select`, `$expand`, `$search` and `$compute` are 501, and so are
//! `/$ref` and `/$count`; `$top`, `$skip`, `$count` and the `ngsild.*` options apply as on any
//! collection read.

use crate::edm::{Member, Set, Shape, Written};
use crate::filter::{
    compute, computed, inlined, mint, numeric, searched, whole, Refused, Translated,
};
use crate::read::{
    attach, compare, digest, entity_value, ngsild, not_implemented, now, page_link, paged,
    position, pushdown, refusal, resource, scanned, set_of, token, Reads,
};
use antares_ops::{Caller, Page as ScanPage, RESIDUAL_SCAN};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use odata_axum::{json_response, ODataErrorResponse};
use odata_json::{
    context_url, write_collection, ContextTarget, Ctx, EntityValue, MetadataLevel, ODataVersion,
    Page,
};
use odata_url::{
    Aggregate, AggregateWith, Compute, Expr, Literal, MemberPath, PartitionBy, QueryOptions,
    Segment, Transformation,
};
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

/// A collection read with `$apply`.
pub(crate) async fn collection(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    (set_name, path): (&str, &[Segment]),
    options: &QueryOptions,
    headers: &HeaderMap,
) -> Result<Response, ODataErrorResponse> {
    let version = ctx.version;
    let beside = options.filter.is_some()
        || options.orderby.is_some()
        || options.select.is_some()
        || options.expand.is_some()
        || options.search.is_some()
        || options.compute.is_some();
    // `/$ref` and `/$count` of the rows
    if beside || path.len() > 1 {
        return Err(not_implemented(
            "$apply beside another query option, under /$ref or /$count",
            version,
        ));
    }
    let set = set_of(reads, set_name, version)?;
    let steps = options.apply.as_deref().unwrap_or_default();
    // every member under a term of the request's context: each step reads the Entities by it
    let mut t = Translated::default();
    for member in set.members.values() {
        mint(&mut t.context, &member.iri);
    }
    ngsild(&reads.view, options, &mut t, set, version)?;
    let program = whole(&t, now()).map_err(|r| refused(r, version))?;
    let pushdown = pushdown(set, None, &[], &mut t);
    let cx = Cx {
        set,
        aliases: &options.aliases,
        context: t.context,
        version,
    };
    let none = Frame::Entities {
        docs: Vec::new(),
        computes: Vec::new(),
    };
    // over no Entity first: a refused transformation never costs a scan
    run(steps, none, &cx)?;
    let all = ScanPage {
        start: 0,
        size: 0,
        whole: true,
    };
    let (docs, _) = scanned(reads, caller, &pushdown, &program, all, version).await?;
    let frame = run(
        steps,
        Frame::Entities {
            docs,
            computes: Vec::new(),
        },
        &cx,
    )?;
    let digest = digest(&reads.state.service_root, path, options);
    let window = position(reads, options, digest, headers, version)?;
    let link = |cursor: u64, skip: u64, top: Option<u64>| {
        page_link(reads, path, options, token(cursor, skip, top, digest))
    };
    let counted = options.count == Some(true);
    match frame {
        Frame::Entities { docs, computes } => {
            let (mut docs, total, next) = paged(docs, window, &link);
            // the `compute` names written with each Entity
            let mut shown = Translated {
                context: cx.context.clone(),
                ..Translated::default()
            };
            // validated as each `compute` came: a refusal here is never reached
            let aliases = &options.aliases;
            compute(&computes, set, aliases, &mut shown, now()).map_err(|r| refused(r, version))?;
            attach(&mut docs, &shown, version)?;
            let page = Page {
                count: counted.then_some(total),
                next_link: next.as_deref(),
            };
            entities(reads, ctx, (set_name, set), &docs, &shown, &page)
        }
        Frame::Rows(rows) => {
            let (values, total, next) = paged(rows.values, window, &link);
            let names: Vec<&str> = rows.names.iter().map(|(n, _)| n.as_str()).collect();
            let count = counted.then_some(total);
            let body = written(ctx, set_name, &names, &values, count, next.as_deref());
            Ok(json_response(version, ctx.level, body))
        }
    }
}

/// What every transformation reads besides its instances: the Entities' set, the request's
/// parameter aliases, the terms the scan read the Entities under, and the version.
struct Cx<'a> {
    set: &'a Set,
    aliases: &'a [(String, Literal)],
    context: Map<String, Value>,
    version: ODataVersion,
}

/// The instances between two transformations.
#[derive(Clone)]
enum Frame {
    /// Entities as the scan read them, with the `compute` items so far over their members.
    Entities {
        docs: Vec<Value>,
        computes: Vec<Compute>,
    },
    /// Rows of an aggregation.
    Rows(Rows),
}

impl Frame {
    /// The `compute` items so far: a row holds its computed values.
    fn computes(&self) -> &[Compute] {
        match self {
            Self::Entities { computes, .. } => computes,
            Self::Rows(_) => &[],
        }
    }

    /// How many instances.
    fn len(&self) -> usize {
        match self {
            Self::Entities { docs, .. } => docs.len(),
            Self::Rows(rows) => rows.values.len(),
        }
    }

    /// The instances at `order`, in that order.
    fn select(self, order: &[usize]) -> Self {
        fn pick<T>(items: Vec<T>, order: &[usize]) -> Vec<T> {
            let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
            order
                .iter()
                .filter_map(|i| slots.get_mut(*i).and_then(Option::take))
                .collect()
        }
        match self {
            Self::Entities { docs, computes } => Self::Entities {
                docs: pick(docs, order),
                computes,
            },
            Self::Rows(rows) => Self::Rows(Rows {
                values: pick(rows.values, order),
                names: rows.names,
            }),
        }
    }
}

/// Rows: each name with the Edm type of its values, and the values of each row in that order.
#[derive(Clone)]
struct Rows {
    names: Vec<(String, &'static str)>,
    values: Vec<Vec<Value>>,
}

/// Each value of each instance: `None` for an unknown one.
type Values = Vec<Vec<Option<Value>>>;

fn run(
    steps: &[Transformation],
    mut frame: Frame,
    cx: &Cx<'_>,
) -> Result<Frame, ODataErrorResponse> {
    for step in steps {
        frame = one(step, frame, cx)?;
    }
    Ok(frame)
}

/// One transformation over `frame`.
fn one(step: &Transformation, frame: Frame, cx: &Cx<'_>) -> Result<Frame, ODataErrorResponse> {
    let step = match (step, &frame) {
        (Transformation::Concat(branches), _) => {
            let computes = frame.computes().to_vec();
            // the branches together hold at most what one scan may read: a chain of `concat`
            // doubles the instances at each step and is refused before it outgrows that (T-4017)
            let mut outs = Vec::with_capacity(branches.len());
            let mut total = 0usize;
            for branch in branches {
                let out = run(branch, frame.clone(), cx)?;
                total = total.saturating_add(out.len());
                if total > RESIDUAL_SCAN {
                    return Err(too_many(cx.version));
                }
                outs.push(out);
            }
            return Ok(joined(outs, computes));
        }
        (Transformation::Compute(items), _) => return computed_into(items, frame, cx),
        // a search is the filter it means (URL 5.1.7, as `$search`)
        (Transformation::Search(search), Frame::Entities { .. }) => {
            Cow::Owned(Transformation::Filter(
                searched(search, cx.set).unwrap_or(Expr::Literal(Literal::Bool(true))),
            ))
        }
        (Transformation::Search(_), Frame::Rows(_)) => {
            return Err(not_implemented(
                "search after aggregate in $apply",
                cx.version,
            ))
        }
        (other, _) => Cow::Borrowed(other),
    };
    let (values, types) = evaluated(&exprs_of(&step), &frame, cx)?;
    if let Some(grouping) = grouped(&step) {
        return aggregated(grouping, &values, &types, cx.version).map(Frame::Rows);
    }
    let order = kept(&step, &values, &types, cx)?;
    Ok(frame.select(&order))
}

/// The grouping properties and the aggregates of a transformation that aggregates (what
/// follows reads its rows), `None` for one that keeps the instances.
fn grouped(step: &Transformation) -> Option<(&[String], &[Aggregate])> {
    match step {
        Transformation::GroupBy {
            properties,
            aggregate,
        } => Some((properties, aggregate.as_deref().unwrap_or_default())),
        Transformation::Aggregate(list) => Some((&[], list)),
        _ => None,
    }
}

/// The expressions a transformation reads of each instance: the grouping properties before
/// the aggregated values (`$count` reads none).
fn exprs_of(step: &Transformation) -> Vec<Expr> {
    let with = |list: &[Aggregate]| -> Vec<Expr> {
        list.iter()
            .filter_map(|a| match a {
                Aggregate::With { expr, .. } => Some(expr.clone()),
                Aggregate::Count { .. } => None,
            })
            .collect()
    };
    match step {
        Transformation::Filter(expr) | Transformation::Partition { expr, .. } => vec![expr.clone()],
        Transformation::OrderBy(keys) => keys.iter().map(|k| k.expr.clone()).collect(),
        Transformation::Aggregate(list) => with(list),
        Transformation::GroupBy {
            properties,
            aggregate,
        } => properties
            .iter()
            .map(|name| {
                Expr::Member(MemberPath {
                    variable: None,
                    path: vec![name.clone()],
                })
            })
            .chain(with(aggregate.as_deref().unwrap_or_default()))
            .collect(),
        _ => Vec::new(),
    }
}

/// The values of `exprs` for each instance of `frame`, and the Edm type of each expression.
fn evaluated(
    exprs: &[Expr],
    frame: &Frame,
    cx: &Cx<'_>,
) -> Result<(Values, Vec<&'static str>), ODataErrorResponse> {
    match frame {
        Frame::Entities { docs, computes } => {
            // a `compute` name is what it computes (URL 5.1.10)
            let exprs: Vec<Expr> = exprs.iter().map(|e| inlined(e, computes)).collect();
            values_of(&exprs, cx.set, cx.context.clone(), docs, cx)
        }
        Frame::Rows(rows) => {
            let set = row_set(&rows.names);
            let mut context = Map::new();
            let terms: Vec<String> = rows
                .names
                .iter()
                .map(|(name, _)| mint(&mut context, &row_iri(name)))
                .collect();
            let docs: Vec<Value> = rows
                .values
                .iter()
                .map(|row| pseudo(&rows.names, &terms, row))
                .collect();
            values_of(exprs, &set, context, &docs, cx)
        }
    }
}

/// The values of `exprs` over the members of `set` for each of `docs`, read under `context`.
fn values_of(
    exprs: &[Expr],
    set: &Set,
    context: Map<String, Value>,
    docs: &[Value],
    cx: &Cx<'_>,
) -> Result<(Values, Vec<&'static str>), ODataErrorResponse> {
    let version = cx.version;
    let items: Vec<Compute> = exprs
        .iter()
        .enumerate()
        .map(|(i, expr)| Compute {
            expr: expr.clone(),
            alias: i.to_string(),
        })
        .collect();
    let mut t = Translated {
        context,
        ..Translated::default()
    };
    compute(&items, set, cx.aliases, &mut t, now()).map_err(|r| refused(r, version))?;
    let programs = computed(&t, now()).map_err(|r| refused(r, version))?;
    let mut values = Values::with_capacity(docs.len());
    for doc in docs {
        let row: Result<Vec<_>, _> = programs
            .iter()
            .map(|(_, p)| p.value(doc).map(|v| v.filter(|v| !v.is_null())))
            .collect();
        values.push(row.map_err(|_| refused(Refused::TooComplex, version))?);
    }
    Ok((values, t.computed_types))
}

/// The `k`th value of instance `i`.
fn at(values: &Values, i: usize, k: usize) -> Option<&Value> {
    values.get(i)?.get(k)?.as_ref()
}

/// The positions of the instances a transformation that does not aggregate keeps, in its
/// order. A sort puts null first ascending and keeps the order of ties (URL 5.1.5).
fn kept(
    step: &Transformation,
    values: &Values,
    types: &[&'static str],
    cx: &Cx<'_>,
) -> Result<Vec<usize>, ODataErrorResponse> {
    let all = 0..values.len();
    let count = |n: &u64| usize::try_from(*n).unwrap_or(usize::MAX);
    Ok(match step {
        Transformation::Filter(_) => all
            .filter(|i| at(values, *i, 0) == Some(&Value::Bool(true)))
            .collect(),
        Transformation::OrderBy(keys) => {
            let mut order: Vec<usize> = all.collect();
            order.sort_by(|a, b| {
                keys.iter()
                    .enumerate()
                    .map(|(k, key)| {
                        let o = compare(at(values, *a, k), at(values, *b, k));
                        if key.descending {
                            o.reverse()
                        } else {
                            o
                        }
                    })
                    .find(|o| o.is_ne())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            order
        }
        Transformation::Top(n) => all.take(count(n)).collect(),
        Transformation::Skip(n) => all.skip(count(n)).collect(),
        Transformation::Partition { by, top, n, .. } => {
            partitioned((*by, *top, n), values, types, cx)?
        }
        // `identity`
        _ => all.collect(),
    })
}

/// `topcount` to `bottompercent`: the instances with a value, highest (`top`) or lowest first,
/// as many as `n` says.
fn partitioned(
    (by, top, n): (PartitionBy, bool, &Expr),
    values: &Values,
    types: &[&'static str],
    cx: &Cx<'_>,
) -> Result<Vec<usize>, ODataErrorResponse> {
    let bad = |message: &str| {
        ODataErrorResponse::new(StatusCode::BAD_REQUEST, "BadRequest", message)
            .with_version(cx.version)
    };
    // the first parameter is one value, read over no member
    let frame = Frame::Entities {
        docs: vec![serde_json::json!({"id": "urn:ngsi-ld:Row:0", "type": "Row"})],
        computes: Vec::new(),
    };
    let (limit, _) = evaluated(std::slice::from_ref(n), &frame, cx)?;
    let limit = at(&limit, 0, 0)
        .and_then(Value::as_f64)
        .filter(|v| *v >= 0.0);
    let limit = limit.ok_or_else(|| bad("the first parameter is a number, not negative"))?;
    if by != PartitionBy::Count && !types.first().is_some_and(|t| numeric(t)) {
        return Err(bad(
            "topsum, bottomsum, toppercent and bottompercent rank numbers",
        ));
    }
    let mut ranked: Vec<usize> = (0..values.len())
        .filter(|i| at(values, *i, 0).is_some())
        .collect();
    ranked.sort_by(|a, b| {
        let o = compare(at(values, *a, 0), at(values, *b, 0));
        if top {
            o.reverse()
        } else {
            o
        }
    });
    let value = |i: &usize| at(values, *i, 0).and_then(Value::as_f64).unwrap_or(0.0);
    let goal = match by {
        PartitionBy::Count => {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::as_conversions
            )]
            let take = limit as usize;
            return Ok(ranked.into_iter().take(take).collect());
        }
        PartitionBy::Sum => limit,
        PartitionBy::Percent => limit / 100.0 * ranked.iter().map(value).sum::<f64>(),
    };
    let mut reached = 0.0;
    let mut kept = Vec::new();
    for i in ranked {
        if reached >= goal {
            break;
        }
        reached += value(&i);
        kept.push(i);
    }
    Ok(kept)
}

/// A `concat` whose branches together hold more than one scan may read (T-4017).
fn too_many(version: ODataVersion) -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::BAD_REQUEST,
        "TooManyResults",
        &format!(
            "the concat answers more than the {RESIDUAL_SCAN} instances one request may read: narrow it with filter"
        ),
    )
    .with_version(version)
}

/// The results of the branches of a `concat`, one after the other: the Entities with the
/// `compute` items before it, or rows under every name any of them has.
fn joined(outs: Vec<Frame>, computes: Vec<Compute>) -> Frame {
    let mut docs = Vec::new();
    let mut tables = Vec::new();
    for out in outs {
        match out {
            Frame::Entities { docs: more, .. } => docs.extend(more),
            Frame::Rows(rows) => tables.push(rows),
        }
    }
    if tables.is_empty() {
        return Frame::Entities { docs, computes };
    }
    let mut names: Vec<(String, &'static str)> = Vec::new();
    for (name, edm) in tables.iter().flat_map(|t| &t.names) {
        if !names.iter().any(|(n, _)| n == name) {
            names.push((name.clone(), edm));
        }
    }
    let values = tables
        .into_iter()
        .flat_map(|table| {
            let names = &names;
            table.values.into_iter().map(move |row| {
                names
                    .iter()
                    .map(|(name, _)| {
                        let i = table.names.iter().position(|(n, _)| n == name);
                        i.and_then(|i| row.get(i).cloned()).unwrap_or(Value::Null)
                    })
                    .collect()
            })
        })
        .collect();
    Frame::Rows(Rows { names, values })
}

/// `compute`: on Entities the items join those so far, each over the Entity's members; on rows
/// each is a new name with its value.
fn computed_into(
    items: &[Compute],
    frame: Frame,
    cx: &Cx<'_>,
) -> Result<Frame, ODataErrorResponse> {
    let exprs: Vec<Expr> = items.iter().map(|i| i.expr.clone()).collect();
    let (values, types) = evaluated(&exprs, &frame, cx)?;
    Ok(match frame {
        Frame::Entities { docs, mut computes } => {
            for item in items {
                let expr = inlined(&item.expr, &computes);
                computes.push(Compute {
                    expr,
                    alias: item.alias.clone(),
                });
            }
            Frame::Entities { docs, computes }
        }
        Frame::Rows(mut rows) => {
            for (item, edm) in items.iter().zip(types) {
                rows.names.push((item.alias.clone(), edm));
            }
            for (row, more) in rows.values.iter_mut().zip(values) {
                row.extend(more.into_iter().map(|v| v.unwrap_or(Value::Null)));
            }
            Frame::Rows(rows)
        }
    })
}

/// The rows of an aggregation, its input `values` in the order [`exprs_of`] gave them with
/// the Edm `types`.
fn aggregated(
    (properties, list): (&[String], &[Aggregate]),
    values: &Values,
    types: &[&'static str],
    version: ODataVersion,
) -> Result<Rows, ODataErrorResponse> {
    let edm_of = |k: usize| types.get(k).copied().unwrap_or("Edm.Untyped");
    let mut names: Vec<(String, &'static str)> = properties
        .iter()
        .enumerate()
        .map(|(k, p)| (p.clone(), edm_of(k)))
        .collect();
    // where each aggregate's input stands among the values, `None` for `$count`
    let mut next = properties.len();
    let mut inputs: Vec<Option<usize>> = Vec::new();
    for a in list {
        let edm = match a {
            Aggregate::Count { .. } => {
                inputs.push(None);
                "Edm.Int64"
            }
            Aggregate::With { with, .. } => {
                let input = edm_of(next);
                inputs.push(Some(next));
                next = next.saturating_add(1);
                match with {
                    AggregateWith::Sum | AggregateWith::Average if !numeric(input) => {
                        return Err(ODataErrorResponse::new(
                            StatusCode::BAD_REQUEST,
                            "BadRequest",
                            "sum and average aggregate numbers",
                        )
                        .with_version(version))
                    }
                    AggregateWith::Sum | AggregateWith::Average => "Edm.Decimal",
                    AggregateWith::CountDistinct => "Edm.Int64",
                    AggregateWith::Min | AggregateWith::Max => input,
                }
            }
        };
        names.push((a.alias().to_owned(), edm));
    }
    // the groups in the order their first instance comes
    let mut groups: Vec<(Vec<Value>, Vec<usize>)> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    if properties.is_empty() {
        groups.push((Vec::new(), (0..values.len()).collect()));
    }
    for i in (0..values.len()).filter(|_| !properties.is_empty()) {
        let key: Vec<Value> = (0..properties.len())
            .map(|k| at(values, i, k).cloned().unwrap_or(Value::Null))
            .collect();
        let text = Value::Array(key.clone()).to_string();
        let group = *index.entry(text).or_insert(groups.len());
        match groups.get_mut(group) {
            Some((_, members)) => members.push(i),
            None => groups.push((key, vec![i])),
        }
    }
    let values = groups
        .into_iter()
        .map(|(mut row, members)| {
            for (a, input) in list.iter().zip(&inputs) {
                let of = members
                    .iter()
                    .filter_map(|i| input.and_then(|k| at(values, *i, k)));
                row.push(folded(a, of, members.len()));
            }
            row
        })
        .collect();
    Ok(Rows { names, values })
}

/// `docs` as the Entities of `set`, written as a collection read writes them.
fn entities(
    reads: &Reads,
    ctx: &Ctx<'_>,
    (set_name, set): (&str, &Set),
    docs: &[Value],
    filter: &Translated,
    page: &Page<'_>,
) -> Result<Response, ODataErrorResponse> {
    let entities: Vec<EntityValue> = docs
        .iter()
        .map(|doc| entity_value(reads, set_name, set, doc, None, &[], filter))
        .collect();
    let resource = resource(reads, set_name, ctx.version)?;
    let mut body = Vec::new();
    // A Vec takes every byte: the write cannot fail.
    let _ = write_collection(ctx, &resource, &entities, page, &mut body);
    Ok(json_response(ctx.version, ctx.level, body))
}

/// One aggregate over the non-null `values` of a group of `instances`.
fn folded<'v>(a: &Aggregate, values: impl Iterator<Item = &'v Value>, instances: usize) -> Value {
    let with = match a {
        Aggregate::Count { .. } => return Value::from(instances),
        Aggregate::With { with, .. } => *with,
    };
    let values: Vec<&Value> = values.collect();
    match with {
        AggregateWith::CountDistinct => {
            let distinct: BTreeSet<String> = values.iter().map(ToString::to_string).collect();
            Value::from(distinct.len())
        }
        AggregateWith::Min | AggregateWith::Max => {
            let pick = values.into_iter().reduce(|best, v| {
                let o = compare(Some(v), Some(best));
                let better = if with == AggregateWith::Min {
                    o.is_lt()
                } else {
                    o.is_gt()
                };
                if better {
                    v
                } else {
                    best
                }
            });
            pick.cloned().unwrap_or(Value::Null)
        }
        AggregateWith::Sum | AggregateWith::Average => {
            let numbers: Vec<&serde_json::Number> =
                values.iter().filter_map(|v| v.as_number()).collect();
            if numbers.is_empty() {
                return Value::Null;
            }
            let float: f64 = numbers.iter().filter_map(|n| n.as_f64()).sum();
            if with == AggregateWith::Average {
                #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
                let n = numbers.len() as f64;
                return serde_json::Number::from_f64(float / n).map_or(Value::Null, Value::Number);
            }
            // integers stay integers while their sum fits
            let int = numbers
                .iter()
                .try_fold(0i64, |sum, n| sum.checked_add(n.as_i64()?));
            int.map_or_else(
                || serde_json::Number::from_f64(float).map_or(Value::Null, Value::Number),
                Value::from,
            )
        }
    }
}

/// The IRI a row's name stands for.
fn row_iri(name: &str) -> String {
    format!("urn:antares:odata:apply:{name}")
}

/// The rows' names as the members of a set, each a single value of its Edm type.
fn row_set(names: &[(String, &'static str)]) -> Set {
    Set {
        output: None,
        type_iri: row_iri("Row"),
        members: names
            .iter()
            .map(|(name, edm)| {
                let member = Member {
                    iri: row_iri(name),
                    shape: Shape::Value {
                        edm,
                        collection: false,
                    },
                    list: false,
                    json: false,
                    untyped: false,
                    written: Written::Never,
                };
                (name.clone(), member)
            })
            .collect(),
        identifier: None,
        writable: crate::edm::Writable::default(),
        references: BTreeMap::new(),
        bound: BTreeMap::new(),
    }
}

/// One row as a normalized NGSI-LD Entity: each non-null value a Property under its term, a
/// date, time or instant as its typed JSON-LD value.
fn pseudo(names: &[(String, &'static str)], terms: &[String], row: &[Value]) -> Value {
    let mut doc = Map::new();
    doc.insert("id".into(), Value::from("urn:ngsi-ld:Row:0"));
    doc.insert("type".into(), Value::from("Row"));
    for (((_, edm), term), v) in names.iter().zip(terms).zip(row) {
        let typed = match *edm {
            "Edm.DateTimeOffset" => Some("DateTime"),
            "Edm.Date" => Some("Date"),
            "Edm.TimeOfDay" => Some("Time"),
            _ => None,
        };
        let v = match (typed, v) {
            (_, Value::Null) => continue,
            (Some(ty), Value::String(_)) => serde_json::json!({"@type": ty, "@value": v}),
            _ => v.clone(),
        };
        doc.insert(
            term.clone(),
            serde_json::json!({"type": "Property", "value": v}),
        );
    }
    Value::Object(doc)
}

/// The rows as a collection: the context URL names the set and the row's names, each row a
/// JSON object of its names (Data Aggregation 4.0 section 3, JSON Format 4.01 section 12).
fn written(
    ctx: &Ctx<'_>,
    set: &str,
    names: &[&str],
    values: &[Vec<Value>],
    count: Option<u64>,
    next: Option<&str>,
) -> Vec<u8> {
    let prefix = match ctx.version {
        ODataVersion::V4_0 => "@odata.",
        ODataVersion::V4_01 => "@",
    };
    let mut members: Vec<(String, Value)> = Vec::new();
    if ctx.level != MetadataLevel::None {
        let target = ContextTarget::Collection {
            set,
            select: Some(names),
        };
        members.push((
            format!("{prefix}context"),
            Value::from(context_url(ctx, &target)),
        ));
    }
    if let Some(count) = count {
        members.push((format!("{prefix}count"), Value::from(count)));
    }
    let rows: Vec<Value> = values
        .iter()
        .map(|row| {
            Value::Object(
                names
                    .iter()
                    .map(|n| (*n).to_owned())
                    .zip(row.iter().cloned())
                    .collect(),
            )
        })
        .collect();
    members.push(("value".into(), Value::Array(rows)));
    if let Some(next) = next {
        members.push((format!("{prefix}nextLink"), Value::from(next)));
    }
    // the members in this order: the control information before `value`, the link after it
    let mut out = b"{".to_vec();
    for (i, (name, v)) in members.into_iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend(Value::from(name).to_string().into_bytes());
        out.push(b':');
        out.extend(v.to_string().into_bytes());
    }
    out.push(b'}');
    out
}

/// The OData error of a `$apply` the surface does not take.
fn refused(refused: Refused, version: ODataVersion) -> ODataErrorResponse {
    refusal(refused, "$apply", version)
}
