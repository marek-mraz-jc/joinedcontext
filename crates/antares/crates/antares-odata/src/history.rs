// SPDX-License-Identifier: EUPL-1.2
//! The `{Set}_history` sets (T-5908), the OData form of the OGC `{type}-history` collections
//! (`docs/src/ogc.md` section 7): beside each class's set of a view that offers the Temporal
//! Query, one row per Entity and stored `observedAt`, keyed by both, read through the view's
//! Query Temporal Evolution of Entities, so the view's Policy and its shown Attributes narrow
//! the history as they narrow the current state. A row's members hold the values valid at its
//! `observedAt`: each Attribute's latest default instance at or before it.
//!
//! `$filter` is evaluated over each row by the residual evaluator; its top-level bounds on
//! `observedAt` narrow the temporal read (`timerel`, `timeAt`, `endTimeAt`), and a lower bound
//! reads each Attribute's last instance before it (`timerel=before&lastN=1`) so the first rows
//! in the window still hold the values valid then. Rows come in Entity then `observedAt` order;
//! `$count` and any other `$orderby` read the whole answer, at most `RESIDUAL_SCAN` rows over
//! `WHOLE_ENTITIES` Entities.
//! ponytail: each Entity's newest [`INSTANCES`] instances per Attribute in the window are read
//! (the broker's own default ceiling), as the OGC history does.

use crate::edm::{Member, Set, OBSERVED_AT_IRI};
use crate::filter::{bounds_of, mint, program, residual_whole, Refused, Translated};
use crate::read::{
    compare, default_instance, encode, not_found, not_implemented, now, page_link, paged, position,
    refusal, resource, run, selected, served, token, Reads,
};
use antares_ops::residual::{DateTime, Program, Truth};
use antares_ops::{Caller, OperationId, RESIDUAL_SCAN};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use odata_axum::{json_response, ODataErrorResponse};
use odata_json::{write_collection, write_entity, Ctx, EntityValue, ODataVersion, Page};
use odata_url::{CompareOp, Expr, Key, KeyValue, Literal, QueryOptions, Segment};
use serde_json::{json, Map, Value};
use std::cmp::Ordering;
use std::fmt::Write as _;

/// The constructs a history set refuses with `501`, named in its message.
const ONE_ROW: &str = "a query option other than $select on one history row";
const UNSERVED: &str = "$expand, $search, $apply or $compute on a history set";
const SELECT: &str = "$select of a navigation, a path or an annotation";
/// The instances of each Attribute one read asks for, the broker's default
/// `ANTARES_TEMPORAL_INSTANCE_LIMIT`.
const INSTANCES: usize = 1_000;
/// The Entities one Temporal Query reads, and the reads one page makes at most; past them a
/// page ends short with its next link.
const BATCH: u64 = 4;
const MAX_READS: usize = 25;
/// The Entities one whole read ([`whole`]: `$count`, a sort) may walk. Each temporal read pages
/// by offset, so a walk costs the square of its Entities: 1 000 is 250 reads, ten natural pages.
const WHOLE_ENTITIES: usize = 1_000;
/// The start of a read no lower bound narrows.
const EPOCH: &str = "1970-01-01T00:00:00Z";

/// An instant in microseconds and as the request wrote it.
type Bound = (i64, String);

/// One row: the Entity, its `observedAt` as stored and in microseconds, and its document in
/// the normalized form the residual reads, each member's term holding the instance valid then.
struct Row {
    urn: String,
    at: String,
    micros: i64,
    doc: Map<String, Value>,
}

/// What a read of a history set asks the broker and keeps.
struct Plan<'s> {
    set: &'s Set,
    filter: Translated,
    program: Option<Program>,
    /// Each member but `observedAt`, with its term.
    terms: Vec<(&'s str, &'s Member, String)>,
    /// The term of `observedAt`.
    observed: String,
    /// The `timerel` part of the temporal query.
    window: String,
    /// The inclusive lower bound, whose earlier instances a second read fills in.
    lower: Option<Bound>,
    /// A window no row can be in.
    empty: bool,
}

pub(crate) async fn collection(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    (set_name, set): (&str, &Set),
    path: &[Segment],
    options: &QueryOptions,
    headers: &HeaderMap,
) -> Result<Response, ODataErrorResponse> {
    let version = ctx.version;
    if path.last() == Some(&Segment::Ref) {
        return Err(not_implemented("$ref of a history set", version));
    }
    let plan = plan(set, options, version)?;
    let picked = picked(set, options, version)?;
    let digest = crate::read::digest(&reads.state.service_root, path, options);
    let (cursor, skip, top, page) = position(reads, options, digest, headers, version)?;
    let link = |cursor: u64, skip: u64, top: Option<u64>| {
        page_link(reads, path, options, token(cursor, skip, top, digest))
    };
    let sort = sort_keys(&plan, options, version)?;
    let counted = options.count == Some(true);
    let (rows, total, next) = if counted || sort.is_some() {
        let mut all = whole(reads, caller, &plan, None, version).await?;
        if let Some(keys) = &sort {
            all.sort_by(|a, b| ordered(keys, a, b));
        }
        paged(all, (cursor, skip, top, page), &link)
    } else {
        let (rows, next) = streamed(
            reads,
            caller,
            &plan,
            (cursor, skip, top, page),
            &link,
            version,
        )
        .await?;
        (rows, 0, next)
    };
    let values: Vec<EntityValue> = rows
        .iter()
        .map(|r| value(set_name, &plan, r, picked.as_deref()))
        .collect();
    let resource = resource(reads, set_name, version)?;
    let names: Option<Vec<&str>> = picked
        .as_ref()
        .map(|p| p.iter().map(String::as_str).collect());
    let resource = match &names {
        Some(names) => resource.select(names),
        None => resource,
    };
    let page = Page {
        count: counted.then_some(total),
        next_link: next.as_deref(),
    };
    let mut body = Vec::new();
    // A Vec takes every byte: the write cannot fail.
    let _ = write_collection(ctx, &resource, &values, &page, &mut body);
    Ok(json_response(version, ctx.level, body))
}

pub(crate) async fn count(
    reads: &Reads,
    caller: &Caller,
    set: &Set,
    options: &QueryOptions,
    version: ODataVersion,
) -> Result<Response, ODataErrorResponse> {
    let plan = plan(set, options, version)?;
    let total = whole(reads, caller, &plan, None, version).await?.len();
    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain")],
        total.to_string(),
    )
        .into_response())
}

/// One row by its key `(id='urn',observedAt=t)`.
pub(crate) async fn entity(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    (set_name, set): (&str, &Set),
    key: &Key,
    options: &QueryOptions,
) -> Result<Response, ODataErrorResponse> {
    let version = ctx.version;
    let Some((urn, micros)) = key_of(key, &options.aliases) else {
        return Err(not_found(version));
    };
    let bare = QueryOptions {
        select: options.select.clone(),
        aliases: options.aliases.clone(),
        ..QueryOptions::default()
    };
    if *options != bare {
        return Err(not_implemented(ONE_ROW, version));
    }
    let plan = plan(set, &bare, version)?;
    let picked = picked(set, options, version)?;
    let rows = whole(reads, caller, &plan, Some(urn), version).await?;
    let row = rows
        .iter()
        .find(|r| r.micros == micros)
        .ok_or_else(|| not_found(version))?;
    let value = value(set_name, &plan, row, picked.as_deref());
    let resource = resource(reads, set_name, version)?;
    let names: Option<Vec<&str>> = picked
        .as_ref()
        .map(|p| p.iter().map(String::as_str).collect());
    let resource = match &names {
        Some(names) => resource.select(names),
        None => resource,
    };
    let mut body = Vec::new();
    // A Vec takes every byte: the write cannot fail.
    let _ = write_entity(ctx, &resource, &value, &mut body);
    Ok(json_response(version, ctx.level, body))
}

/// The id and `observedAt` (in microseconds) a key names, through the request's aliases.
fn key_of<'k>(key: &'k Key, aliases: &'k [(String, Literal)]) -> Option<(&'k str, i64)> {
    let Key::Compound(parts) = key else {
        return None;
    };
    let value = |name: &str| {
        let v = parts.iter().find(|(n, _)| n == name).map(|(_, v)| v)?;
        match v {
            KeyValue::Literal(l) => Some(l),
            KeyValue::Alias(a) => aliases.iter().find(|(n, _)| n == a).map(|(_, l)| l),
        }
    };
    match (parts.len(), value("id")?, value("observedAt")?) {
        (2, Literal::String(urn), Literal::DateTimeOffset(at)) => {
            Some((urn, DateTime::parse(at)?.unix_micros()))
        }
        _ => None,
    }
}

/// The request as a [`Plan`]: `$expand`, `$search`, `$apply`, `$compute` and the `ngsild.`
/// options are not served on a history.
fn plan<'s>(
    set: &'s Set,
    options: &QueryOptions,
    version: ODataVersion,
) -> Result<Plan<'s>, ODataErrorResponse> {
    let unserved = [
        options.expand.is_some(),
        options.search.is_some(),
        options.apply.is_some(),
        options.compute.is_some(),
    ];
    if unserved.contains(&true) {
        return Err(not_implemented(UNSERVED, version));
    }
    if !options.custom.is_empty() {
        return Err(ODataErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "BadRequest",
            "a history set takes no ngsild option",
        )
        .with_version(version));
    }
    let mut filter = match &options.filter {
        Some(f) => {
            residual_whole(f, set, &options.aliases).map_err(|r| refusal(r, "$filter", version))?
        }
        None => Translated::default(),
    };
    let program = program(&filter, now()).map_err(|r| refusal(r, "$filter", version))?;
    let terms = set
        .members
        .iter()
        .filter(|(_, m)| m.iri != OBSERVED_AT_IRI)
        .map(|(name, m)| (name.as_str(), m, mint(&mut filter.context, &m.iri)))
        .collect();
    let observed = mint(&mut filter.context, OBSERVED_AT_IRI);
    let bounds: Vec<(CompareOp, Bound)> = options
        .filter
        .as_ref()
        .map(|f| bounds_of(f, "observedAt", &options.aliases))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(op, at)| Some((op, (DateTime::parse(at)?.unix_micros(), at.to_owned()))))
        .collect();
    let lower = bounds
        .iter()
        .filter(|(op, _)| matches!(op, CompareOp::Ge | CompareOp::Gt | CompareOp::Eq))
        .map(|(_, b)| b.clone())
        .max_by_key(|(micros, _)| *micros);
    let upper = bounds
        .iter()
        .filter(|(op, _)| matches!(op, CompareOp::Lt))
        .map(|(_, b)| b.clone())
        .min_by_key(|(micros, _)| *micros);
    let empty = matches!((&lower, &upper), (Some((l, _)), Some((u, _))) if l >= u);
    let window = match (&lower, &upper) {
        (Some((_, l)), Some((_, u))) => format!(
            "timerel=between&timeAt={}&endTimeAt={}",
            encode(l),
            encode(u)
        ),
        (Some((_, l)), None) => format!("timerel=after&timeAt={}", encode(l)),
        (None, Some((_, u))) => format!("timerel=before&timeAt={}", encode(u)),
        (None, None) => format!("timerel=after&timeAt={EPOCH}"),
    };
    Ok(Plan {
        set,
        filter,
        program,
        terms,
        observed,
        window,
        lower,
        empty,
    })
}

/// The members `$select` names (the key always among them, Protocol 11.2.5.1), `None` for
/// every one; a path, a navigation or an annotation is not served.
fn picked(
    set: &Set,
    options: &QueryOptions,
    version: ODataVersion,
) -> Result<Option<Vec<String>>, ODataErrorResponse> {
    let names =
        selected(set, options.select.as_ref()).map_err(|()| not_implemented(SELECT, version))?;
    Ok(names.map(|names| {
        let asked = names
            .into_iter()
            .filter(|n| *n == "id" || set.members.contains_key(*n));
        let mut out: Vec<String> = ["id", "observedAt"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        out.extend(
            asked
                .filter(|n| !matches!(*n, "id" | "observedAt"))
                .map(str::to_owned),
        );
        out
    }))
}

/// What a sort key reads from a row.
enum By<'s> {
    Id,
    Observed,
    Member(&'s Member, String),
}

/// `$orderby` as keys over the rows, `None` for the natural order (Entity, then
/// `observedAt` ascending), which a page streams. A key that is no single-valued member is
/// `400`.
fn sort_keys<'s>(
    plan: &Plan<'s>,
    options: &QueryOptions,
    version: ODataVersion,
) -> Result<Option<Vec<(By<'s>, bool)>>, ODataErrorResponse> {
    let Some(items) = &options.orderby else {
        return Ok(None);
    };
    let mut keys = Vec::new();
    for item in items {
        let name = match &item.expr {
            Expr::Member(m) => m.path.join("/"),
            _ => String::new(),
        };
        let member = plan
            .terms
            .iter()
            .find(|(n, m, _)| *n == name && m.shape.sortable());
        let by = match (name.as_str(), member) {
            ("id", _) => By::Id,
            ("observedAt", _) => By::Observed,
            (_, Some((_, m, term))) => By::Member(m, term.clone()),
            (_, None) => return Err(refusal(Refused::Bad, "$orderby", version)),
        };
        keys.push((by, item.descending));
    }
    let natural = matches!(
        keys.as_slice(),
        [(By::Id, false)] | [(By::Id, false), (By::Observed, false)]
    );
    Ok((!natural).then_some(keys))
}

/// `a` against `b` by `keys`, then in the natural order.
fn ordered(keys: &[(By<'_>, bool)], a: &Row, b: &Row) -> Ordering {
    let by = |row: &Row, key: &By<'_>| -> Value {
        match key {
            By::Id => Value::from(row.urn.as_str()),
            By::Observed => Value::from(row.micros),
            By::Member(m, term) => served(&m.shape, default_instance(row.doc.get(term))).0,
        }
    };
    keys.iter()
        .map(|(key, descending)| {
            let o = compare(Some(&by(a, key)), Some(&by(b, key)));
            if *descending {
                o.reverse()
            } else {
                o
            }
        })
        .find(|o| o.is_ne())
        .unwrap_or_else(|| a.urn.cmp(&b.urn).then(a.micros.cmp(&b.micros)))
}

/// Every row of the answer (of the one Entity `urn`): at most [`RESIDUAL_SCAN`] rows kept and
/// [`WHOLE_ENTITIES`] Entities read, as a residual scan is bounded (ADR-0028), so a filter that
/// keeps no row never walks the whole type.
async fn whole(
    reads: &Reads,
    caller: &Caller,
    plan: &Plan<'_>,
    urn: Option<&str>,
    version: ODataVersion,
) -> Result<Vec<Row>, ODataErrorResponse> {
    let mut all = Vec::new();
    let mut offset = 0;
    loop {
        let (batch, more) = batch(reads, caller, plan, urn, offset, version).await?;
        all.extend(batch.into_iter().flatten());
        offset = offset.saturating_add(BATCH);
        let scanned = usize::try_from(offset).unwrap_or(usize::MAX);
        if all.len() > RESIDUAL_SCAN || (more && scanned >= WHOLE_ENTITIES) {
            return Err(ODataErrorResponse::new(
                StatusCode::BAD_REQUEST,
                "TooManyResults",
                &format!(
                    "the history holds more than the {RESIDUAL_SCAN} rows or {WHOLE_ENTITIES} Entities one count or sort may read: narrow it with $filter on id or observedAt"
                ),
            )
            .with_version(version));
        }
        if !more {
            return Ok(all);
        }
    }
}

/// One page in the natural order: Entities read [`BATCH`] at a time from the cursor's Entity
/// offset, `skip` rows dropped first. The next link resumes at the batch the page ended in.
async fn streamed(
    reads: &Reads,
    caller: &Caller,
    plan: &Plan<'_>,
    (cursor, mut skip, top, page): (u64, u64, Option<u64>, u64),
    link: &(dyn Fn(u64, u64, Option<u64>) -> String + Sync),
    version: ODataVersion,
) -> Result<(Vec<Row>, Option<String>), ODataErrorResponse> {
    let want = top.map_or(page, |top| top.min(page));
    let mut out: Vec<Row> = Vec::new();
    let mut offset = cursor;
    for _ in 0..MAX_READS {
        let (batch, more) = batch(reads, caller, plan, None, offset, version).await?;
        let rows: Vec<Row> = batch.into_iter().flatten().collect();
        let held = u64::try_from(rows.len()).unwrap_or(u64::MAX);
        let start = skip;
        let mut used = start;
        for row in rows
            .into_iter()
            .skip(usize::try_from(start).unwrap_or(usize::MAX))
        {
            if u64::try_from(out.len()).unwrap_or(u64::MAX) >= want {
                break;
            }
            out.push(row);
            used = used.saturating_add(1);
        }
        skip = skip.saturating_sub(held);
        let served = u64::try_from(out.len()).unwrap_or(u64::MAX);
        let left = top.map(|top| top.saturating_sub(served));
        if left == Some(0) {
            return Ok((out, None));
        }
        if served >= want {
            let next = if used < held {
                Some(link(offset, used, left))
            } else {
                more.then(|| link(offset.saturating_add(BATCH), 0, left))
            };
            return Ok((out, next));
        }
        if !more {
            return Ok((out, None));
        }
        offset = offset.saturating_add(BATCH);
    }
    let served = u64::try_from(out.len()).unwrap_or(u64::MAX);
    let left = top.map(|top| top.saturating_sub(served));
    Ok((out, Some(link(offset, skip, left))))
}

/// The rows of [`BATCH`] Entities from `offset` (or of the one `urn`), Entity by Entity, and
/// whether more Entities may follow.
async fn batch(
    reads: &Reads,
    caller: &Caller,
    plan: &Plan<'_>,
    urn: Option<&str>,
    offset: u64,
    version: ODataVersion,
) -> Result<(Vec<Vec<Row>>, bool), ODataErrorResponse> {
    if plan.empty {
        return Ok((Vec::new(), false));
    }
    let attrs: Vec<&str> = plan
        .terms
        .iter()
        .map(|(_, _, term)| term.as_str())
        .collect();
    let ids = urn
        .map(|u| vec![u.to_owned()])
        .or_else(|| plan.filter.ids.clone());
    let read = |window: &str, last: usize, ids: Option<&[String]>, offset: u64| {
        let mut target = format!(
            "/ngsi-ld/v1/temporal/entities?type={}&{window}&lastN={last}&limit={BATCH}&offset={offset}",
            encode(&plan.set.type_iri)
        );
        let _ = write!(target, "&attrs={}", encode(&attrs.join(",")));
        if let Some(ids) = ids {
            let _ = write!(target, "&id={}", encode(&ids.join(",")));
        }
        target
    };
    let target = read(&plan.window, INSTANCES, ids.as_deref(), offset);
    let (mut entities, _) = run(
        reads,
        caller,
        OperationId::TemporalQuery,
        target,
        &plan.filter,
        version,
    )
    .await?;
    let more = urn.is_none() && entities.len() >= usize::try_from(BATCH).unwrap_or(usize::MAX);
    if let (Some((_, lower)), false) = (&plan.lower, entities.is_empty()) {
        let held: Vec<String> = entities
            .iter()
            .filter_map(|e| e.get("id")?.as_str().map(str::to_owned))
            .collect();
        let before = format!("timerel=before&timeAt={}", encode(lower));
        let target = read(&before, 1, Some(&held), 0);
        let (earlier, _) = run(
            reads,
            caller,
            OperationId::TemporalQuery,
            target,
            &plan.filter,
            version,
        )
        .await?;
        for e in &mut entities {
            let id = e.get("id").and_then(Value::as_str).map(str::to_owned);
            let Some(old) = earlier
                .iter()
                .find(|o| o.get("id").and_then(Value::as_str) == id.as_deref())
            else {
                continue;
            };
            merge(e, old, &attrs);
        }
    }
    let rows = entities
        .iter()
        .map(|e| rows(plan, e, version))
        .collect::<Result<_, _>>()?;
    Ok((rows, more))
}

/// `e` with the instances of each of `attrs` that `old` holds added.
fn merge(e: &mut Value, old: &Value, attrs: &[&str]) {
    for term in attrs {
        let earlier = instances_of(old.get(*term));
        if earlier.is_empty() {
            continue;
        }
        let mut now: Vec<Value> = instances_of(e.get(*term)).into_iter().cloned().collect();
        now.extend(earlier.into_iter().cloned());
        let list = Value::Array(now);
        e.as_object_mut()
            .into_iter()
            .for_each(|obj| drop(obj.insert((*term).to_owned(), list.clone())));
    }
}

/// The instances of a temporal Attribute: a list, or one object unlisted.
fn instances_of(attr: Option<&Value>) -> Vec<&Value> {
    match attr {
        Some(Value::Array(list)) => list.iter().collect(),
        Some(one @ Value::Object(_)) => vec![one],
        _ => Vec::new(),
    }
}

/// An instance's `observedAt` in microseconds and as stored, and the instance.
type Timed<'v> = (i64, &'v str, &'v Value);

/// The default instances (no `datasetId`) of one Attribute with their `observedAt`, oldest
/// first; an instance without `observedAt` has no place in a history.
fn timed(attr: Option<&Value>) -> Vec<Timed<'_>> {
    let mut out: Vec<Timed<'_>> = instances_of(attr)
        .into_iter()
        .filter(|i| i.get("datasetId").is_none())
        .filter_map(|i| {
            let at = i.get("observedAt")?.as_str()?;
            Some((DateTime::parse(at)?.unix_micros(), at, i))
        })
        .collect();
    out.sort_by_key(|(t, _, _)| *t);
    out
}

/// The rows of one temporal Entity, oldest first: one per distinct `observedAt` at or past the
/// lower bound, kept when the residual holds; refused when the residual is past its bounds.
fn rows(plan: &Plan<'_>, e: &Value, version: ODataVersion) -> Result<Vec<Row>, ODataErrorResponse> {
    let urn = e.get("id").and_then(Value::as_str).unwrap_or_default();
    let attrs: Vec<(&str, Vec<Timed<'_>>)> = plan
        .terms
        .iter()
        .map(|(_, _, term)| (term.as_str(), timed(e.get(term.as_str()))))
        .collect();
    let mut times: Vec<(i64, &str)> = attrs
        .iter()
        .flat_map(|(_, list)| list.iter().map(|(t, text, _)| (*t, *text)))
        .filter(|(t, _)| plan.lower.as_ref().is_none_or(|(l, _)| t >= l))
        .collect();
    times.sort_by_key(|(t, _)| *t);
    times.dedup_by_key(|(t, _)| *t);
    let mut out = Vec::new();
    for (at, text) in times {
        let mut doc = Map::new();
        doc.insert("id".into(), Value::from(urn));
        doc.insert("type".into(), Value::from(plan.set.type_iri.as_str()));
        for (term, list) in &attrs {
            if let Some((_, _, instance)) = list.iter().rev().find(|(t, _, _)| *t <= at) {
                doc.insert((*term).to_owned(), (*instance).clone());
            }
        }
        doc.insert(
            plan.observed.clone(),
            json!({"type": "Property", "value": {"@type": "DateTime", "@value": text}}),
        );
        let doc = Value::Object(doc);
        let keep = match &plan.program {
            None => true,
            Some(p) => {
                p.eval(&doc)
                    .map_err(|_| refusal(Refused::TooComplex, "$filter", version))?
                    == Truth::True
            }
        };
        if let (true, Value::Object(doc)) = (keep, doc) {
            out.push(Row {
                urn: urn.to_owned(),
                at: text.to_owned(),
                micros: at,
                doc,
            });
        }
    }
    Ok(out)
}

/// A row as its OData entity: the key, then each member (the `picked` ones) as the current
/// state serves it.
fn value(set_name: &str, plan: &Plan<'_>, row: &Row, picked: Option<&[String]>) -> EntityValue {
    let wanted = |name: &str| picked.is_none_or(|p| p.iter().any(|n| n == name));
    let mut properties = vec![
        ("id".to_owned(), Value::from(row.urn.as_str())),
        ("observedAt".to_owned(), Value::from(row.at.as_str())),
    ];
    for (name, member, term) in plan.terms.iter().filter(|(name, _, _)| wanted(name)) {
        let instance = default_instance(row.doc.get(term.as_str()));
        let (value, _) = served(&member.shape, instance);
        properties.push(((*name).to_owned(), value));
    }
    let key = Key::Compound(vec![
        (
            "id".into(),
            KeyValue::Literal(Literal::String(row.urn.clone())),
        ),
        (
            "observedAt".into(),
            KeyValue::Literal(Literal::DateTimeOffset(row.at.clone())),
        ),
    ]);
    EntityValue {
        id: format!("{set_name}{}", Segment::Key(key)),
        properties,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unlisted_instance_is_one_and_only_timed_default_instances_count() {
        let one = json!({"type": "Property", "value": 4, "observedAt": "2026-10-21T10:00:00Z"});
        assert_eq!(instances_of(Some(&one)).len(), 1);
        assert_eq!(instances_of(Some(&json!(3))).len(), 0);
        let list = json!([
            {"type": "Property", "value": 1, "datasetId": "urn:x", "observedAt": "2026-10-21T10:00:00Z"},
            {"type": "Property", "value": 2},
            {"type": "Property", "value": 3, "observedAt": "2026-10-21T11:00:00+01:00"},
            {"type": "Property", "value": 5, "observedAt": "2026-10-21T09:00:00Z"}
        ]);
        let got: Vec<&str> = timed(Some(&list))
            .into_iter()
            .map(|(_, at, _)| at)
            .collect();
        assert_eq!(got, ["2026-10-21T09:00:00Z", "2026-10-21T11:00:00+01:00"]);
    }

    #[test]
    fn merge_adds_the_earlier_instances_of_each_attribute() {
        let mut e = json!({"id": "urn:a", "t0": {"type": "Property", "value": 2, "observedAt": "2026-10-21T10:00:00Z"}});
        let old = json!({"id": "urn:a", "t0": [{"type": "Property", "value": 1, "observedAt": "2026-10-21T09:00:00Z"}]});
        merge(&mut e, &old, &["t0", "t1"]);
        assert_eq!(timed(e.get("t0")).len(), 2);
        assert!(
            e.get("t1").is_none(),
            "an attribute with no earlier instance stays absent"
        );
    }
}
