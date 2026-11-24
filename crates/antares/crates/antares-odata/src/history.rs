// SPDX-License-Identifier: EUPL-1.2
//! The `{Set}_history` sets (T-5908), the OData form of the OGC `{type}-history` collections
//! (`docs/src/ogc.md` section 7): beside each class's set of a view that offers the Temporal
//! Query, one row per Entity and stored `observedAt`, keyed by both, read through the view's
//! Query Temporal Evolution of Entities, so the view's Policy and its shown Attributes narrow
//! the history as they narrow the current state. A row's members hold the values valid at its
//! `observedAt`: each Attribute's latest default instance at or before it.
//!
//! Rows are built lazily along one time axis per Entity (T-7518): an instance stays the broker's
//! JSON until a row that is judged or served holds it, a page builds its own rows only, and what
//! it builds is charged to the broker's answer pool beside the answers it reads.
//!
//! `$filter` is evaluated over each row by the residual evaluator; its top-level bounds on
//! `observedAt` narrow the temporal read (`timerel`, `timeAt`, `endTimeAt`), and a lower bound
//! reads each Attribute's last instance before it (`timerel=before&lastN=1`) so the first rows
//! in the window still hold the values valid then. Rows come in Entity then `observedAt` order;
//! A page judges at most [`PAGE_JUDGED`] rows by `$filter` and holds at most [`PAGE_BYTES`];
//! past either it ends with a next link at that row. `$count` and any other `$orderby` read the
//! whole answer, at most `RESIDUAL_SCAN` rows over `WHOLE_ENTITIES` Entities, and keep no row
//! past the page's end.
//! ponytail: each Entity's newest [`INSTANCES`] instances per Attribute in the window are read
//! (the broker's own default ceiling), as the OGC history does, and read again by each page
//! inside that Entity. Paging by time needs a `firstN` the Postgres store serves from its index
//! (`newest_rows` serves `lastN` alone; T-5616).

use crate::edm::{Member, Set, OBSERVED_AT_IRI};
use crate::filter::{bounds_of, mint, program, residual_whole, Refused, Translated};
use crate::read::{
    answered, bad_token, compare, default_instance, encode, get, next_page, not_found,
    not_implemented, now, page_link, position, refusal, refused, resource, selected, served, token,
    Reads, ODATA_VERSION,
};
use antares_ops::bounds::{AnswerCharge, HeldAnswer};
use antares_ops::residual::{DateTime, Program, Truth};
use antares_ops::{Caller, OperationId, RESIDUAL_SCAN};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use odata_axum::{json_response, ODataErrorResponse};
use odata_json::{write_collection, write_entity, Ctx, EntityValue, ODataVersion, Page};
use odata_url::{CompareOp, Expr, Key, KeyValue, Literal, QueryOptions, Segment};
use serde::de::IgnoredAny;
use serde_json::value::RawValue;
use serde_json::{json, Map, Value};
use std::cmp::Ordering;
use std::collections::HashMap;
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
/// The rows one page judges by `$filter` at most, of one Entity or of several.
const PAGE_JUDGED: usize = RESIDUAL_SCAN;
/// The bytes of rows one page holds at most, never more than half of what one answer may hold:
/// a value repeats in every row it is valid in.
const PAGE_BYTES: usize = 8 * 1024 * 1024;
/// The rows walked between two yields to the runtime.
const YIELD_EVERY: usize = 1_024;
/// The start of a read no lower bound narrows.
const EPOCH: &str = "1970-01-01T00:00:00Z";

/// An instant in microseconds and as the request wrote it.
type Bound = (i64, String);

/// What a read of a history set asks the broker and keeps.
struct Plan<'s> {
    set: &'s Set,
    filter: Translated,
    program: Option<Program>,
    /// Each member but `observedAt`, with its term.
    terms: Vec<(&'s str, &'s Member, String)>,
    /// Each member's term once, in the members' order: the Attributes a row holds.
    attrs: Vec<String>,
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
    let sort = sort_keys(&plan, options, version)?;
    let counted = options.count == Some(true);
    let link = |text: String| page_link(reads, path, options, text);
    let scan = Scan {
        reads,
        caller,
        plan: &plan,
        set_name,
        picked: picked.as_deref(),
        version,
    };
    let (built, total, next) = if counted || sort.is_some() {
        let (cursor, skip, top, page) = position(reads, options, digest, headers, version)?;
        let from = cursor.saturating_add(skip);
        let kept = whole(&scan, sort.as_deref(), from, page).await?;
        let served = u64::try_from(kept.built.values.len()).unwrap_or(u64::MAX);
        let at = (cursor, skip, top, page);
        let next = next_page(served, kept.total, at, &|cursor, skip, top| {
            link(token(cursor, skip, top, digest))
        });
        (kept.built, kept.total, next)
    } else {
        let start = start(reads, options, digest, headers, version)?;
        let (built, next) = streamed(&scan, start).await?;
        (built, 0, next.map(|next| link(next.token(digest))))
    };
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
    let _ = write_collection(ctx, &resource, &built.values, &page, &mut body);
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
    let scan = Scan {
        reads,
        caller,
        plan: &plan,
        // a count serves no row
        set_name: "",
        picked: None,
        version,
    };
    let total = whole(&scan, None, 0, 0).await?.total;
    Ok((
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "text/plain"),
            (ODATA_VERSION, version.as_str()),
        ],
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
    let mut plan = plan(set, &bare, version)?;
    // the row's time ends the read: the newest instances before it are the ones it holds
    let Some((_, end)) = just_after(micros) else {
        return Err(not_found(version));
    };
    plan.window = format!("timerel=before&timeAt={}", encode(&end));
    let picked = picked(set, options, version)?;
    let scan = Scan {
        reads,
        caller,
        plan: &plan,
        set_name,
        picked: picked.as_deref(),
        version,
    };
    let value = row_at(&scan, urn, micros)
        .await?
        .ok_or_else(|| not_found(version))?;
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

/// The exclusive bound one microsecond after `micros`, with its text.
fn just_after(micros: i64) -> Option<Bound> {
    let next = micros.checked_add(1)?;
    Some((next, DateTime::from_unix_micros(next)?.to_string()))
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
        Some(f) => match residual_whole(f, set, &options.aliases) {
            Ok(filter) => filter,
            Err(r) => return Err(refusal(r, "$filter", version)),
        },
        None => Translated::default(),
    };
    let program = match program(&filter, now()) {
        Ok(program) => program,
        Err(r) => return Err(refusal(r, "$filter", version)),
    };
    let terms: Vec<(&str, &Member, String)> = set
        .members
        .iter()
        .filter(|(_, m)| m.iri != OBSERVED_AT_IRI)
        .map(|(name, m)| (name.as_str(), m, mint(&mut filter.context, &m.iri)))
        .collect();
    let observed = mint(&mut filter.context, OBSERVED_AT_IRI);
    let mut named = std::collections::HashSet::new();
    let attrs = terms
        .iter()
        .filter(|(_, _, term)| named.insert(term.as_str()))
        .map(|(_, _, term)| term.clone())
        .collect();
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
    // `endTimeAt` is exclusive: `le` and `eq` end one microsecond past their time
    let upper = bounds
        .iter()
        .filter_map(|(op, b)| match op {
            CompareOp::Lt => Some(b.clone()),
            CompareOp::Le | CompareOp::Eq => just_after(b.0),
            _ => None,
        })
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
        attrs,
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

/// What one request's rows are built with.
struct Scan<'p> {
    reads: &'p Reads,
    caller: &'p Caller,
    plan: &'p Plan<'p>,
    set_name: &'p str,
    /// The members `$select` names.
    picked: Option<&'p [String]>,
    version: ODataVersion,
}

impl Scan<'_> {
    /// Whether the residual keeps the row `axis` is at; refused when it is past its bounds.
    fn keeps(&self, axis: &mut Axis<'_>) -> Result<bool, ODataErrorResponse> {
        let Some(program) = &self.plan.program else {
            return Ok(true);
        };
        let truth = program
            .eval(axis.doc(self.version)?)
            .map_err(|_| refusal(Refused::TooComplex, "$filter", self.version))?;
        Ok(truth == Truth::True)
    }

    /// The row `axis` is at as its OData entity, and its bytes.
    fn value(&self, axis: &mut Axis<'_>) -> Result<(EntityValue, usize), ODataErrorResponse> {
        axis.hold(self.version)?;
        let value = value(
            self.set_name,
            self.plan,
            (axis.urn, axis.at, &axis.doc),
            self.picked,
        );
        let bytes = size_of_row(&value);
        Ok((value, bytes))
    }
}

/// Rows built, holding their share of the broker's answer pool until they are written.
struct Built {
    values: Vec<EntityValue>,
    charge: AnswerCharge,
}

impl Built {
    fn new(reads: &Reads) -> Self {
        Self {
            values: Vec::new(),
            charge: reads.view.answer_share(),
        }
    }
}

/// Widens `charge` to `bytes`, `503` while the pool has no room.
async fn charged(
    charge: &mut AnswerCharge,
    bytes: usize,
    version: ODataVersion,
) -> Result<(), ODataErrorResponse> {
    charge
        .grow(bytes, "odata")
        .await
        .map_err(|_| refused(StatusCode::SERVICE_UNAVAILABLE, version))
}

/// A row's bytes: its members' names and JSON, and its id twice (the member and the key).
fn size_of_row(row: &EntityValue) -> usize {
    struct Bytes(usize);
    impl std::io::Write for Bytes {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut bytes = Bytes(row.id.len().saturating_mul(2));
    for (name, value) in &row.properties {
        bytes.0 = bytes
            .0
            .saturating_add(name.len())
            .saturating_add(std::mem::size_of::<(String, Value)>());
        // Counting takes every byte: the write cannot fail.
        let _ = serde_json::to_writer(&mut bytes, value);
    }
    bytes.0
}

/// Lets the runtime serve others: a walk is all CPU between two reads.
async fn breathe() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if yielded {
            return std::task::Poll::Ready(());
        }
        yielded = true;
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    })
    .await;
}

/// A row a whole read keeps, with what orders it.
struct Candidate {
    keys: Vec<Value>,
    urn: String,
    micros: i64,
    value: EntityValue,
    bytes: usize,
}

/// `a` against `b` by `keys`, then in the natural order.
fn ordered(keys: &[(By<'_>, bool)], a: &Candidate, b: &Candidate) -> Ordering {
    keys.iter()
        .zip(a.keys.iter().zip(&b.keys))
        .map(|((_, descending), (a, b))| {
            let o = compare(Some(a), Some(b));
            if *descending {
                o.reverse()
            } else {
                o
            }
        })
        .find(|o| o.is_ne())
        .unwrap_or_else(|| a.urn.cmp(&b.urn).then(a.micros.cmp(&b.micros)))
}

/// What a whole read answers: how many rows there are and the page of them.
struct Whole {
    total: u64,
    built: Built,
}

fn too_many(detail: &str, version: ODataVersion) -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::BAD_REQUEST,
        "TooManyResults",
        &format!(
            "the history holds more than {detail}: narrow it with $filter on id or observedAt"
        ),
    )
    .with_version(version)
}

/// The count of the whole answer and its rows `from..from + page`: at most [`RESIDUAL_SCAN`]
/// rows counted and [`WHOLE_ENTITIES`] Entities read, as a residual scan is bounded (ADR-0028),
/// so a filter that keeps no row never walks the whole type. A count builds no row; a sort
/// keeps no more than twice the rows up to the page's end.
async fn whole(
    scan: &Scan<'_>,
    sort: Option<&[(By<'_>, bool)]>,
    from: u64,
    page: u64,
) -> Result<Whole, ODataErrorResponse> {
    let (reads, version) = (scan.reads, scan.version);
    let end = from.saturating_add(page);
    // a page that starts past every row an answer may count holds none
    let last = usize::try_from(end)
        .unwrap_or(usize::MAX)
        .min(RESIDUAL_SCAN);
    let most = reads.view.max_answer_bytes();
    let mut built = Built::new(reads);
    let mut kept: Vec<Candidate> = Vec::new();
    let (mut total, mut held, mut walked) = (0u64, 0usize, 0usize);
    let mut offset = 0;
    loop {
        let batch = Batch::read(scan, None, offset).await?;
        let found = batch.entities(version)?;
        let more = found.len() >= usize::try_from(BATCH).unwrap_or(usize::MAX);
        for entity in &found {
            breathe().await;
            let mut axis = Axis::new(scan.plan, entity, version)?;
            while axis.step() {
                walked = walked.saturating_add(1);
                if walked % YIELD_EVERY == 0 {
                    breathe().await;
                }
                if !scan.keeps(&mut axis)? {
                    continue;
                }
                let index = total;
                total = total.saturating_add(1);
                if total > u64::try_from(RESIDUAL_SCAN).unwrap_or(u64::MAX) {
                    return Err(too_many(
                        &format!("the {RESIDUAL_SCAN} rows one count or sort may read"),
                        version,
                    ));
                }
                let wanted = match sort {
                    Some(_) => page > 0,
                    None => (from..end).contains(&index),
                };
                if !wanted {
                    continue;
                }
                let keys = sort_values(sort.unwrap_or_default(), &mut axis, version)?;
                let (value, bytes) = scan.value(&mut axis)?;
                held = held.saturating_add(bytes);
                kept.push(Candidate {
                    keys,
                    urn: axis.urn.to_owned(),
                    micros: axis.micros,
                    value,
                    bytes,
                });
                if kept.len() >= last.saturating_mul(2) || held > most {
                    if let Some(keys) = sort {
                        kept.sort_by(|a, b| ordered(keys, a, b));
                        kept.truncate(last);
                        held = kept.iter().map(|c| c.bytes).sum();
                    }
                }
                if held > most {
                    return Err(too_many(
                        &format!("the {most} bytes one count or sort may hold"),
                        version,
                    ));
                }
                charged(&mut built.charge, held, version).await?;
            }
        }
        offset = offset.saturating_add(BATCH);
        if !more {
            break;
        }
        if usize::try_from(offset).unwrap_or(usize::MAX) >= WHOLE_ENTITIES {
            return Err(too_many(
                &format!("the {WHOLE_ENTITIES} Entities one count or sort may read"),
                version,
            ));
        }
    }
    let mut skipped = 0;
    if let Some(keys) = sort {
        kept.sort_by(|a, b| ordered(keys, a, b));
        skipped = usize::try_from(from).unwrap_or(usize::MAX);
    }
    built.values = kept
        .into_iter()
        .skip(skipped)
        .take(usize::try_from(page).unwrap_or(usize::MAX))
        .map(|c| c.value)
        .collect();
    Ok(Whole { total, built })
}

/// What `keys` read from the row `axis` is at.
fn sort_values(
    keys: &[(By<'_>, bool)],
    axis: &mut Axis<'_>,
    version: ODataVersion,
) -> Result<Vec<Value>, ODataErrorResponse> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    axis.hold(version)?;
    let by = |key: &By<'_>| match key {
        By::Id => Value::from(axis.urn),
        By::Observed => Value::from(axis.micros),
        By::Member(m, term) => served(&m.shape, default_instance(axis.doc.get(term.as_str()))).0,
    };
    Ok(keys.iter().map(|(key, _)| by(key)).collect())
}

/// The row of the Entity `urn` at `micros`.
async fn row_at(
    scan: &Scan<'_>,
    urn: &str,
    micros: i64,
) -> Result<Option<EntityValue>, ODataErrorResponse> {
    let batch = Batch::read(scan, Some(urn), 0).await?;
    for entity in &batch.entities(scan.version)? {
        let mut axis = Axis::new(scan.plan, entity, scan.version)?;
        while axis.step() {
            if axis.micros == micros {
                return Ok(Some(scan.value(&mut axis)?.0));
            }
        }
    }
    Ok(None)
}

/// Where a page starts and how much it takes.
#[derive(Debug, Clone, Copy)]
struct Start {
    /// The Entities before the page's first read.
    offset: u64,
    /// The rows of that read's Entities earlier pages went past, judged or not.
    passed: u64,
    /// The kept rows still to drop (`$skip`).
    skip: u64,
    top: Option<u64>,
    page: u64,
}

impl Start {
    /// The skip token of a page that starts here: the plain one at a read's first row, `3.`
    /// inside a read's rows.
    fn token(&self, digest: u64) -> String {
        if self.passed == 0 {
            return token(self.offset, self.skip, self.top, digest);
        }
        let top = self.top.map_or_else(|| "-".to_owned(), |t| t.to_string());
        let (offset, passed, skip) = (self.offset, self.passed, self.skip);
        format!("3.{offset}.{passed}.{skip}.{top}.{digest:016x}")
    }
}

/// The page's [`Start`] from the request's options or its skip token. The token is no secret
/// and grants nothing: every page reads through the view and its Policy again.
fn start(
    reads: &Reads,
    options: &QueryOptions,
    digest: u64,
    headers: &HeaderMap,
    version: ODataVersion,
) -> Result<Start, ODataErrorResponse> {
    let inside = options
        .skiptoken
        .as_deref()
        .and_then(|text| text.strip_prefix("3."));
    let Some(text) = inside else {
        let (offset, skip, top, page) = position(reads, options, digest, headers, version)?;
        return Ok(Start {
            offset,
            passed: 0,
            skip,
            top,
            page,
        });
    };
    if options.skip.is_some() || options.top.is_some() {
        return Err(bad_token(version));
    }
    let (offset, passed, skip, top) =
        read_inside(text, digest).ok_or_else(|| bad_token(version))?;
    let resumed = QueryOptions {
        skiptoken: None,
        skip: Some(skip),
        top,
        ..options.clone()
    };
    let (_, skip, top, page) = position(reads, &resumed, digest, headers, version)?;
    Ok(Start {
        offset,
        passed,
        skip,
        top,
        page,
    })
}

/// The fields of a `3.` token of this request.
fn read_inside(text: &str, digest: u64) -> Option<(u64, u64, u64, Option<u64>)> {
    let parts: Vec<&str> = text.split('.').collect();
    let [offset, passed, skip, top, tag] = parts.as_slice() else {
        return None;
    };
    if *tag != format!("{digest:016x}") {
        return None;
    }
    let top = match *top {
        "-" => None,
        n => Some(n.parse().ok()?),
    };
    Some((
        offset.parse().ok()?,
        passed.parse().ok()?,
        skip.parse().ok()?,
        top,
    ))
}

/// One page in the natural order: Entities read [`BATCH`] at a time from the start's Entity
/// offset, `skip` kept rows dropped first. It ends at the rows it wants, at [`PAGE_JUDGED`]
/// rows judged, at [`PAGE_BYTES`] or at [`MAX_READS`], and the next link resumes at the row it
/// stopped before.
async fn streamed(
    scan: &Scan<'_>,
    start: Start,
) -> Result<(Built, Option<Start>), ODataErrorResponse> {
    let (reads, version) = (scan.reads, scan.version);
    let want = start.top.map_or(start.page, |top| top.min(start.page));
    let most = PAGE_BYTES.min(reads.view.max_answer_bytes() / 2);
    let mut built = Built::new(reads);
    let mut at = start;
    let (mut judged, mut walked, mut held) = (0usize, 0usize, 0usize);
    for _ in 0..MAX_READS {
        let batch = Batch::read(scan, None, at.offset).await?;
        let found = batch.entities(version)?;
        if at.top == Some(0) {
            return Ok((built, None));
        }
        let mut row = 0u64;
        for entity in &found {
            breathe().await;
            let mut axis = Axis::new(scan.plan, entity, version)?;
            while axis.step() {
                let here = row;
                row = row.saturating_add(1);
                walked = walked.saturating_add(1);
                if walked % YIELD_EVERY == 0 {
                    breathe().await;
                }
                if here < at.passed {
                    continue;
                }
                let stop = Start { passed: here, ..at };
                if scan.plan.program.is_some() {
                    if judged >= PAGE_JUDGED {
                        return Ok((built, Some(stop)));
                    }
                    judged = judged.saturating_add(1);
                }
                if !scan.keeps(&mut axis)? {
                    continue;
                }
                if at.skip > 0 {
                    at.skip = at.skip.saturating_sub(1);
                    continue;
                }
                let full = u64::try_from(built.values.len()).unwrap_or(u64::MAX) >= want;
                if full {
                    return Ok((built, Some(stop)));
                }
                let (value, bytes) = scan.value(&mut axis)?;
                // one row always goes out, or the next link would name this page again
                if !built.values.is_empty() && held.saturating_add(bytes) > most {
                    return Ok((built, Some(stop)));
                }
                held = held.saturating_add(bytes);
                charged(&mut built.charge, held, version).await?;
                built.values.push(value);
                at.top = at.top.map(|top| top.saturating_sub(1));
                if at.top == Some(0) {
                    return Ok((built, None));
                }
            }
        }
        if found.len() < usize::try_from(BATCH).unwrap_or(usize::MAX) {
            return Ok((built, None));
        }
        at.offset = at.offset.saturating_add(BATCH);
        at.passed = 0;
        if u64::try_from(built.values.len()).unwrap_or(u64::MAX) >= want {
            return Ok((built, Some(at)));
        }
    }
    Ok((built, Some(at)))
}

/// The temporal answers of [`BATCH`] Entities, held in the answer pool while rows are built
/// from them: their instances, and past a lower bound each one's last before it.
struct Batch {
    window: Option<HeldAnswer>,
    earlier: Option<HeldAnswer>,
}

/// One Entity of a [`Batch`], unparsed.
struct Found<'a> {
    urn: String,
    window: &'a RawValue,
    earlier: Option<&'a RawValue>,
}

impl Batch {
    /// The Entities from `offset`, or the one `urn`.
    async fn read(
        scan: &Scan<'_>,
        urn: Option<&str>,
        offset: u64,
    ) -> Result<Self, ODataErrorResponse> {
        let plan = scan.plan;
        if plan.empty {
            return Ok(Self {
                window: None,
                earlier: None,
            });
        }
        let ids = urn
            .map(|u| vec![u.to_owned()])
            .or_else(|| plan.filter.ids.clone());
        let window = temporal(scan, &plan.window, INSTANCES, ids.as_deref(), offset).await?;
        let mut earlier = None;
        if let Some((_, lower)) = &plan.lower {
            let held: Vec<String> = elements(window.as_ref(), scan.version)?
                .into_iter()
                .filter_map(id_of)
                .collect();
            if !held.is_empty() {
                let before = format!("timerel=before&timeAt={}", encode(lower));
                earlier = temporal(scan, &before, 1, Some(&held), 0).await?;
            }
        }
        Ok(Self { window, earlier })
    }

    fn entities(&self, version: ODataVersion) -> Result<Vec<Found<'_>>, ODataErrorResponse> {
        let earlier: Vec<(String, &RawValue)> = elements(self.earlier.as_ref(), version)?
            .into_iter()
            .filter_map(|e| Some((id_of(e)?, e)))
            .collect();
        Ok(elements(self.window.as_ref(), version)?
            .into_iter()
            .map(|window| {
                let urn = id_of(window).unwrap_or_default();
                let earlier = earlier.iter().find(|(id, _)| *id == urn).map(|(_, e)| *e);
                Found {
                    urn,
                    window,
                    earlier,
                }
            })
            .collect())
    }
}

/// One Temporal Query of the set's Attributes over `window`, `last` instances of each.
async fn temporal(
    scan: &Scan<'_>,
    window: &str,
    last: usize,
    ids: Option<&[String]>,
    offset: u64,
) -> Result<Option<HeldAnswer>, ODataErrorResponse> {
    let plan = scan.plan;
    let mut target = format!(
        "/ngsi-ld/v1/temporal/entities?type={}&{window}&lastN={last}&limit={BATCH}&offset={offset}",
        encode(&plan.set.type_iri)
    );
    let _ = write!(target, "&attrs={}", encode(&plan.attrs.join(",")));
    if let Some(ids) = ids {
        let _ = write!(target, "&id={}", encode(&ids.join(",")));
    }
    let (answer, _) = answered(
        scan.reads,
        scan.caller,
        OperationId::TemporalQuery,
        get(target, &plan.filter),
        scan.version,
    )
    .await?;
    Ok(answer)
}

/// The elements of an answer's array, each still its JSON.
fn elements(
    answer: Option<&HeldAnswer>,
    version: ODataVersion,
) -> Result<Vec<&RawValue>, ODataErrorResponse> {
    let Some(bytes) = answer else {
        return Ok(Vec::new());
    };
    // the store answered this JSON itself
    let Ok(elements) = serde_json::from_slice(bytes) else {
        return Err(refused(StatusCode::INTERNAL_SERVER_ERROR, version));
    };
    Ok(elements)
}

fn id_of(entity: &RawValue) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Id {
        id: String,
    }
    serde_json::from_str::<Id>(entity.get()).ok().map(|e| e.id)
}

/// The instances of a temporal Attribute: a list, or one object unlisted.
fn instances_of(attr: &RawValue) -> Vec<&RawValue> {
    match attr.get().as_bytes().first() {
        Some(b'[') => serde_json::from_str(attr.get()).unwrap_or_default(),
        Some(b'{') => vec![attr],
        _ => Vec::new(),
    }
}

/// What places an instance on the axis.
#[derive(serde::Deserialize)]
struct Head<'a> {
    #[serde(rename = "observedAt", borrow)]
    at: Option<&'a str>,
    #[serde(rename = "datasetId")]
    dataset: Option<IgnoredAny>,
}

/// A default instance (no `datasetId`) on the axis; an instance without `observedAt` has no
/// place in a history.
struct Timed<'a> {
    micros: i64,
    at: &'a str,
    slot: usize,
    instance: &'a RawValue,
}

fn timed(instance: &RawValue, slot: usize) -> Option<Timed<'_>> {
    let head: Head<'_> = serde_json::from_str(instance.get()).ok()?;
    if head.dataset.is_some() {
        return None;
    }
    let at = head.at?;
    Some(Timed {
        micros: DateTime::parse(at)?.unix_micros(),
        at,
        slot,
        instance,
    })
}

/// One Entity's rows, oldest first: one merged time axis over its Attributes' instances, sorted
/// once (O(I log I) for I instances), and the instance of each Attribute valid where it stands.
/// A step moves references; an instance is parsed when a row that holds it is judged or served.
struct Axis<'a> {
    plan: &'a Plan<'a>,
    urn: &'a str,
    times: Vec<Timed<'a>>,
    next: usize,
    /// The row it stands at.
    micros: i64,
    at: &'a str,
    /// Per Attribute, the instance valid at the row, and the one `doc` holds.
    valid: Vec<Option<&'a RawValue>>,
    parsed: Vec<Option<&'a RawValue>>,
    /// The normalized document the residual and the members read.
    doc: Map<String, Value>,
    judged: Value,
}

impl<'a> Axis<'a> {
    fn new(
        plan: &'a Plan<'a>,
        entity: &'a Found<'a>,
        version: ODataVersion,
    ) -> Result<Self, ODataErrorResponse> {
        // the members of the store's own answer
        fn members(
            raw: &RawValue,
            version: ODataVersion,
        ) -> Result<HashMap<String, &RawValue>, ODataErrorResponse> {
            let Ok(members) = serde_json::from_str(raw.get()) else {
                return Err(refused(StatusCode::INTERNAL_SERVER_ERROR, version));
            };
            Ok(members)
        }
        let window = members(entity.window, version)?;
        let earlier = match entity.earlier {
            Some(raw) => members(raw, version)?,
            None => HashMap::new(),
        };
        let mut times: Vec<Timed<'a>> = Vec::new();
        for (slot, term) in plan.attrs.iter().enumerate() {
            for list in [window.get(term), earlier.get(term)].into_iter().flatten() {
                times.extend(
                    instances_of(list)
                        .into_iter()
                        .filter_map(|i| timed(i, slot)),
                );
            }
        }
        // Stable: of one instant the members' order and the answer's stand, so the row is
        // spelled as the first and holds, per Attribute, the last.
        times.sort_by_key(|t| t.micros);
        Ok(Self {
            plan,
            urn: &entity.urn,
            times,
            next: 0,
            micros: 0,
            at: "",
            valid: vec![None; plan.attrs.len()],
            parsed: vec![None; plan.attrs.len()],
            doc: Map::new(),
            judged: Value::Null,
        })
    }

    /// Moves to the next distinct `observedAt` at or past the lower bound; false past the last.
    fn step(&mut self) -> bool {
        while let Some(first) = self.times.get(self.next) {
            let (micros, at) = (first.micros, first.at);
            while let Some(t) = self.times.get(self.next).filter(|t| t.micros == micros) {
                if let Some(valid) = self.valid.get_mut(t.slot) {
                    *valid = Some(t.instance);
                }
                self.next = self.next.saturating_add(1);
            }
            if self.plan.lower.as_ref().is_none_or(|(l, _)| micros >= *l) {
                (self.micros, self.at) = (micros, at);
                return true;
            }
        }
        false
    }

    /// Brings `doc` to the row: only the instances that changed since are parsed.
    fn hold(&mut self, version: ODataVersion) -> Result<(), ODataErrorResponse> {
        if let Value::Object(doc) = std::mem::take(&mut self.judged) {
            self.doc = doc;
        }
        let changed = self
            .plan
            .attrs
            .iter()
            .zip(&self.valid)
            .zip(&mut self.parsed);
        for ((term, valid), parsed) in changed {
            let Some(instance) = *valid else { continue };
            if parsed.is_some_and(|held| std::ptr::eq(held, instance)) {
                continue;
            }
            // an instance of the store's own answer
            let Ok(value) = serde_json::from_str::<Value>(instance.get()) else {
                return Err(refused(StatusCode::INTERNAL_SERVER_ERROR, version));
            };
            self.doc.insert(term.clone(), value);
            *parsed = Some(instance);
        }
        Ok(())
    }

    /// The row as the document the residual judges.
    fn doc(&mut self, version: ODataVersion) -> Result<&Value, ODataErrorResponse> {
        self.hold(version)?;
        let doc = &mut self.doc;
        if !doc.contains_key("id") {
            doc.insert("id".into(), Value::from(self.urn));
            doc.insert("type".into(), Value::from(self.plan.set.type_iri.as_str()));
        }
        doc.insert(
            self.plan.observed.clone(),
            json!({"type": "Property", "value": {"@type": "DateTime", "@value": self.at}}),
        );
        self.judged = Value::Object(std::mem::take(doc));
        Ok(&self.judged)
    }
}

/// A row as its OData entity: the key, then each member (the `picked` ones) as the current
/// state serves it.
fn value(
    set_name: &str,
    plan: &Plan<'_>,
    (urn, at, doc): (&str, &str, &Map<String, Value>),
    picked: Option<&[String]>,
) -> EntityValue {
    let wanted = |name: &str| picked.is_none_or(|p| p.iter().any(|n| n == name));
    let mut properties = vec![
        ("id".to_owned(), Value::from(urn)),
        ("observedAt".to_owned(), Value::from(at)),
    ];
    for (name, member, term) in plan.terms.iter().filter(|(name, _, _)| wanted(name)) {
        let instance = default_instance(doc.get(term.as_str()));
        let (value, _) = served(&member.shape, instance);
        properties.push(((*name).to_owned(), value));
    }
    let key = Key::Compound(vec![
        (
            "id".into(),
            KeyValue::Literal(Literal::String(urn.to_owned())),
        ),
        (
            "observedAt".into(),
            KeyValue::Literal(Literal::DateTimeOffset(at.to_owned())),
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

    fn raw(value: &Value) -> Box<RawValue> {
        serde_json::value::to_raw_value(value).unwrap_or_default()
    }

    #[test]
    fn an_unlisted_instance_is_one_and_only_timed_default_instances_count() {
        let one =
            raw(&json!({"type": "Property", "value": 4, "observedAt": "2026-10-21T10:00:00Z"}));
        assert_eq!(instances_of(&one).len(), 1);
        assert_eq!(instances_of(&raw(&json!(3))).len(), 0);
        let list = raw(&json!([
            {"type": "Property", "value": 1, "datasetId": "urn:x", "observedAt": "2026-10-21T10:00:00Z"},
            {"type": "Property", "value": 2},
            {"type": "Property", "value": 3, "observedAt": "2026-10-21T11:00:00+01:00"},
            {"type": "Property", "value": 5, "observedAt": "2026-10-21T09:00:00Z"}
        ]));
        let mut got: Vec<Timed<'_>> = instances_of(&list)
            .into_iter()
            .filter_map(|i| timed(i, 0))
            .collect();
        got.sort_by_key(|t| t.micros);
        let at: Vec<&str> = got.iter().map(|t| t.at).collect();
        assert_eq!(at, ["2026-10-21T09:00:00Z", "2026-10-21T11:00:00+01:00"]);
    }

    /// A row as the test compares it: its spelling, time and document.
    type Seen = (String, i64, Value);
    /// An instance the eager build placed: its time, spelling and document.
    type Placed = (i64, String, Value);

    /// The rows as `rows` built them before T-7518, eagerly and by scanning every list for
    /// every time: the reference the axis answers like.
    fn eager(plan: &Plan<'_>, window: &Value, earlier: &Value) -> Vec<Seen> {
        let list = |e: &Value, term: &str| match e.get(term) {
            Some(Value::Array(list)) => list.clone(),
            Some(one @ Value::Object(_)) => vec![one.clone()],
            _ => Vec::new(),
        };
        let attrs: Vec<(&str, Vec<Placed>)> = plan
            .terms
            .iter()
            .map(|(_, _, term)| {
                let mut out: Vec<Placed> = list(window, term)
                    .into_iter()
                    .chain(list(earlier, term))
                    .filter(|i| i.get("datasetId").is_none())
                    .filter_map(|i| {
                        let at = i.get("observedAt")?.as_str()?.to_owned();
                        Some((DateTime::parse(&at)?.unix_micros(), at, i))
                    })
                    .collect();
                out.sort_by_key(|(t, _, _)| *t);
                (term.as_str(), out)
            })
            .collect();
        let mut times: Vec<(i64, String)> = attrs
            .iter()
            .flat_map(|(_, list)| list.iter().map(|(t, text, _)| (*t, text.clone())))
            .filter(|(t, _)| plan.lower.as_ref().is_none_or(|(l, _)| t >= l))
            .collect();
        times.sort_by_key(|(t, _)| *t);
        times.dedup_by_key(|(t, _)| *t);
        times
            .into_iter()
            .map(|(at, text)| {
                let mut doc = Map::new();
                for (term, list) in &attrs {
                    if let Some((_, _, instance)) = list.iter().rev().find(|(t, _, _)| *t <= at) {
                        doc.insert((*term).to_owned(), instance.clone());
                    }
                }
                (text, at, Value::Object(doc))
            })
            .collect()
    }

    proptest::proptest! {
        /// Ties, two spellings of one instant, datasets, unlisted instances, two members of
        /// one term and a lower bound with earlier instances: each row of the axis is the
        /// eager one, whether every row or only some are parsed.
        #[test]
        fn the_axis_answers_the_rows_the_eager_build_did(
            stored in proptest::collection::vec(
                (0usize..4, 0i64..8, 0u8..3, proptest::bool::ANY, proptest::bool::ANY), 0..24),
            lower in proptest::option::of(0i64..8),
            every in 1usize..4,
        ) {
            let member = Member {
                iri: String::new(),
                shape: crate::edm::Shape::Geo,
                list: false,
                json: false,
                untyped: false,
                written: crate::edm::Written::Always,
            };
            let set = Set {
                type_iri: String::new(),
                members: std::collections::BTreeMap::new(),
                identifier: None,
                writable: crate::edm::Writable::default(),
                references: std::collections::BTreeMap::new(),
                bound: std::collections::BTreeMap::new(),
                output: None,
            };
            let term = |n: usize| format!("t{}", n.min(2));
            let plan = Plan {
                set: &set,
                filter: Translated::default(),
                program: None,
                terms: (0..4).map(|n| ("m", &member, term(n))).collect(),
                attrs: (0..3).map(term).collect(),
                observed: "observed".to_owned(),
                window: String::new(),
                lower: lower.map(|l| (l.saturating_mul(3_600_000_000), String::new())),
                empty: false,
            };
            let (mut window, mut earlier) = (json!({"id": "urn:a"}), json!({"id": "urn:a"}));
            for (n, (attr, hour, kind, offset, before)) in stored.iter().enumerate() {
                let at = if *offset {
                    format!("1970-01-01T{:02}:00:00+01:00", hour.saturating_add(1))
                } else {
                    format!("1970-01-01T{hour:02}:00:00Z")
                };
                let mut i = json!({"type": "Property", "value": n, "observedAt": at});
                if let (1, Some(fields)) = (*kind, i.as_object_mut()) {
                    fields.insert("datasetId".into(), json!("urn:d"));
                }
                let side = if *before { &mut earlier } else { &mut window };
                if let Some(members) = side.as_object_mut() {
                    let held = members.entry(term(*attr)).or_insert_with(|| json!([]));
                    if let Some(list) = held.as_array_mut() {
                        list.push(i);
                    }
                }
            }
            let (w, e) = (raw(&window), raw(&earlier));
            let found = Found { urn: "urn:a".to_owned(), window: &w, earlier: Some(&e) };
            let version = ODataVersion::V4_01;
            let want = eager(&plan, &window, &earlier);
            let Ok(mut axis) = Axis::new(&plan, &found, version) else {
                return Err(proptest::test_runner::TestCaseError::fail("axis"));
            };
            let mut row = 0usize;
            while axis.step() {
                let Some((text, micros, doc)) = want.get(row) else {
                    return Err(proptest::test_runner::TestCaseError::fail("a row too many"));
                };
                proptest::prop_assert_eq!((axis.at, axis.micros), (text.as_str(), *micros));
                if row.is_multiple_of(every) {
                    proptest::prop_assert!(axis.hold(version).is_ok());
                    proptest::prop_assert_eq!(&Value::Object(axis.doc.clone()), doc);
                }
                row = row.saturating_add(1);
            }
            proptest::prop_assert_eq!(row, want.len());
        }
    }

    #[test]
    fn a_token_inside_a_read_answers_its_own_request_alone() {
        let at = Start {
            offset: 8,
            passed: 41,
            skip: 3,
            top: Some(9),
            page: 100,
        };
        let text = at.token(7);
        let inside = text.strip_prefix("3.").unwrap_or_default();
        assert_eq!(read_inside(inside, 7), Some((8, 41, 3, Some(9))));
        assert_eq!(read_inside(inside, 8), None, "another request's token");
        assert_eq!(read_inside(&format!("{inside}.1"), 7), None);
        let first = Start { passed: 0, ..at };
        assert_eq!(first.token(7), token(8, 3, Some(9), 7));
    }
}
