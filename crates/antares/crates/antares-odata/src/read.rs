// SPDX-License-Identifier: EUPL-1.2
//! Reads (odata-linkml-mapping.md 5.1): an EntitySet (Q29) as Query
//! Entities, one Entity by key (Q30) as Retrieve Entity, `/$count` (Q13),
//! with `$filter` and `$orderby` ([`crate::filter`]), `$select`, `$top`,
//! `$skip` and `$count`. Each request is one
//! operation through `ViewOps::run`; the NGSI-LD answer is mapped back
//! through the model, so a member the model does not hold is never written.
//! The options and forms of later rounds answer `501` before any operation
//! runs.

use crate::edm::{base_member, Edm, Set, Shape, VOCABULARY};
use crate::filter::{
    compute, computed, holds_member, inlined, mint, order, program, searched, translate, Refused,
    SortBy, SortKey, Translated,
};
use antares_ops::bounds::HeldAnswer;
use antares_ops::residual::{base64url, decode_base64, DateTime, Expr as RExpr, Program};
use antares_ops::{
    core_name, pickable, Caller, OperationId, OpsRequest, Page as ScanPage, Pushdown, Refusal,
    Scanned, ViewOps, RESIDUAL_SCAN, TOO_COMPLEX,
};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use odata_axum::{json_response, ODataErrorResponse, ODataState};
use odata_json::{
    write_collection_end, write_collection_entity, write_collection_start, write_entity,
    write_entity_references, Ctx, EntityValue, ODataVersion, Page, Resource,
};
use odata_url::{
    EdmModel, ExpandKind, ExpandPath, Expr, Key, KeyValue, Literal, ODataUri, QueryOptions,
    Segment, SelectItem,
};
use serde_json::Value;
use std::sync::Arc;

/// The page size when neither `$top` nor `odata.maxpagesize` is smaller.
pub const DEFAULT_PAGE: u64 = 100;

/// The largest page: the NGSI-LD `limit` bound of the broker.
pub const MAX_PAGE: u64 = 1000;

/// The custom query options a collection read takes (ADR-0028 rule 6, Q27):
/// each the NGSI-LD Query Entities parameter after the prefix.
pub const NGSILD_OPTIONS: [&str; 9] = [
    "ngsild.lang",
    "ngsild.q",
    "ngsild.scopeQ",
    "ngsild.georel",
    "ngsild.geometry",
    "ngsild.coordinates",
    "ngsild.geoproperty",
    "ngsild.pick",
    "ngsild.omit",
];

/// What a read needs besides the request.
pub(crate) struct Reads {
    pub(crate) view: ViewOps,
    pub(crate) edm: Arc<Edm>,
    pub(crate) state: Arc<ODataState>,
}

/// `$entity?$id=` (Q23): the id, percent-decoded, is a URL of this service,
/// absolute under its root or relative to it, that names one Entity by key
/// and nothing else; it is read as that URL is. A missing `$id`, a broken
/// escape or an id with query options of its own is `400`; any other id
/// names no Entity here and is `404`, like a hidden one (O5). A foreign URL
/// is never fetched.
pub(crate) async fn dereference(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    options: &QueryOptions,
) -> Result<Response, ODataErrorResponse> {
    let id = options
        .id
        .as_deref()
        .and_then(percent_decoded)
        .ok_or_else(|| {
            ODataErrorResponse::new(
                StatusCode::BAD_REQUEST,
                "BadRequest",
                "$id is missing or not percent-encoded",
            )
            .with_version(ctx.version)
        })?;
    let relative = id.strip_prefix(ctx.service_root).unwrap_or(&id);
    let model = EdmModel(reads.state.url_model());
    let parsed = (!relative.starts_with('/') && !relative.contains("://"))
        .then(|| odata_url::parse(relative, &model, &reads.state.url_limits));
    match parsed {
        Some(Ok(ODataUri::Resource { options, .. })) if options != QueryOptions::default() => {
            Err(ODataErrorResponse::new(
                StatusCode::BAD_REQUEST,
                "BadRequest",
                "$id has query options of its own",
            )
            .with_version(ctx.version))
        }
        Some(Ok(ODataUri::Resource { path, options })) => match path.as_slice() {
            [Segment::EntitySet(set), Segment::Key(key)] => {
                entity(reads, caller, ctx, set, key, &options, &HeaderMap::new()).await
            }
            _ => Err(not_found(ctx.version)),
        },
        _ => Err(not_found(ctx.version)),
    }
}

use antares_ops::percent::decoded as percent_decoded;

/// Whether `$select` or `$expand` asks for properties.
pub(crate) fn shaped(options: &QueryOptions) -> bool {
    options.select.is_some() || options.expand.is_some()
}

/// A reference has no properties to select or expand (URL 4.4).
pub(crate) fn no_properties(version: ODataVersion) -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::BAD_REQUEST,
        "BadRequest",
        "an entity reference has no properties to $select or $expand",
    )
    .with_version(version)
}

/// Whether no option but `$select` (and `$format`, `$schemaversion`,
/// read before) is asked.
pub(crate) fn only_select(options: &QueryOptions) -> bool {
    options.filter.is_none()
        && options.search.is_none()
        && options.compute.is_none()
        && options.apply.is_none()
        && options.expand.is_none()
        && options.orderby.is_none()
        && options.top.is_none()
        && options.skip.is_none()
        && options.count.is_none()
        && options.skiptoken.is_none()
}

/// Why a parsed URL is answered `501` in this round: a form or an option a
/// later round maps. `None` for what this round serves.
pub(crate) fn later(uri: &ODataUri, schema_version: &str) -> Option<&'static str> {
    let ODataUri::Resource { options, .. } = uri else {
        return Some("this resource");
    };
    let refused = [
        (options.id.is_some(), "$id outside $entity"),
        (
            options
                .schemaversion
                .as_deref()
                .is_some_and(|v| v != "*" && v != schema_version),
            "a $schemaversion other than the served one",
        ),
    ];
    refused
        .iter()
        .find(|(asked, _)| *asked)
        .map(|(_, name)| *name)
}

/// The members a `$select` list names, `None` for every member. A path
/// through a complex property, a navigation property or an annotation is a
/// later round's.
pub(crate) fn selected<'s>(
    set: &'s Set,
    select: Option<&'s Vec<SelectItem>>,
) -> Result<Option<Vec<&'s str>>, ()> {
    selected_of(set, select, &[])
}

/// [`selected`], the `$compute` names of the request among the members.
fn selected_of<'s>(
    set: &'s Set,
    select: Option<&'s Vec<SelectItem>>,
    computed: &[(String, RExpr)],
) -> Result<Option<Vec<&'s str>>, ()> {
    let Some(items) = select else {
        return Ok(None);
    };
    let mut names = Vec::new();
    for item in items {
        match item {
            SelectItem::Star => return Ok(None),
            SelectItem::Path(path) => match path.as_slice() {
                [name] if base_member(name) || computed.iter().any(|(c, _)| c == name) => {
                    names.push(name.as_str());
                }
                [name]
                    if set
                        .members
                        .get(name)
                        .is_some_and(|m| !matches!(m.shape, Shape::Nav { .. })) =>
                {
                    names.push(name.as_str());
                }
                _ => return Err(()),
            },
            SelectItem::Annotation(_) => return Err(()),
        }
    }
    Ok(Some(names))
}

/// What one `$expand` item writes.
enum Writes<'s> {
    /// `/$ref`: an entity reference per target, under this id member.
    Refs(&'static str),
    /// The targets, projected by the nested `$select` (`None` for all).
    Entities(Option<Vec<&'s str>>),
}

/// One navigation member `$expand` names, one level deep (Q3, Q6 to Q8).
pub(crate) struct Expansion<'s> {
    name: &'s str,
    iri: &'s str,
    target_name: &'s str,
    target: &'s Set,
    collection: bool,
    writes: Writes<'s>,
}

/// The `$expand` items of a request. `*` names every navigation member,
/// a named item overrides it. A nested `$expand`, `$levels` or a path
/// through a complex property is a later round's.
fn expanded<'s>(
    edm: &'s Edm,
    set: &'s Set,
    options: &'s QueryOptions,
    version: ODataVersion,
) -> Result<Vec<Expansion<'s>>, ()> {
    let mut out: Vec<Expansion<'s>> = Vec::new();
    for item in options.expand.iter().flatten() {
        let named = match &item.path {
            ExpandPath::Star => None,
            ExpandPath::Path(path) => match path.as_slice() {
                [name] => Some(name.as_str()),
                _ => return Err(()),
            },
        };
        let select = match &item.kind {
            ExpandKind::Ref => None,
            ExpandKind::Entities {
                select,
                expand: None,
                levels: None,
            } => select.as_ref(),
            ExpandKind::Entities { .. } => return Err(()),
        };
        for (name, member) in &set.members {
            let Shape::Nav { target, collection } = &member.shape else {
                continue;
            };
            if named.is_some_and(|n| n != name) {
                continue;
            }
            if named.is_none() && out.iter().any(|e| e.name == name) {
                continue;
            }
            out.retain(|e| e.name != name);
            let target_set = edm.sets.get(target).ok_or(())?;
            let writes = match item.kind {
                ExpandKind::Ref => Writes::Refs(match version {
                    ODataVersion::V4_0 => "@odata.id",
                    ODataVersion::V4_01 => "@id",
                }),
                ExpandKind::Entities { .. } => Writes::Entities(selected(target_set, select)?),
            };
            out.push(Expansion {
                name,
                iri: &member.iri,
                target_name: target,
                target: target_set,
                collection: *collection,
                writes,
            });
        }
    }
    Ok(out)
}

/// The names the context URL lists (JSON Format 10.9): the selected ones
/// and each expanded member, `owner()` or `owner(name)`; 4.0 lists an
/// expansion only with a nested `$select`.
fn listed(picked: Option<&[&str]>, expansions: &[Expansion], version: ODataVersion) -> Vec<String> {
    let mut names: Vec<String> = picked
        .into_iter()
        .flatten()
        .map(|n| (*n).to_owned())
        .collect();
    for e in expansions {
        match (&e.writes, version) {
            (Writes::Entities(Some(nested)), _) => {
                names.push(format!("{}({})", e.name, nested.join(",")));
            }
            (Writes::Entities(None), ODataVersion::V4_01) => names.push(format!("{}()", e.name)),
            _ => {}
        }
    }
    names
}

/// The NGSI-LD query of an EntitySet: its type, the Attributes selected
/// (`pick`, a projection that keeps every Entity, unlike `attrs`; by IRI,
/// with the core members and the expanded Relationships; none when an IRI
/// is no 4.21 member name, and the surface projects), the system members
/// only when selected (E6),
/// `join=inline` one level deep for an expansion of entities or a `q` that
/// follows a Relationship (5.7.2.4). A join
/// follows every Relationship it is given (4.5.23), so it names the
/// structural members and the expanded ones: a navigation not expanded
/// spends none of the walk's lookups.
pub(crate) fn query(
    set: &Set,
    picked: Option<&[&str]>,
    expansions: &[Expansion],
    filter: &Translated,
) -> String {
    let nested = nested(expansions);
    let joined = joins(expansions, filter);
    let frame: Vec<&str>;
    let picked = match picked {
        None if joined => {
            frame = structural(set);
            Some(frame.as_slice())
        }
        picked => picked,
    };
    let mut q = selection(set, filter);
    if let Some(keys) = &filter.order {
        q.push_str("&orderBy=");
        q.push_str(&encode(keys));
    }
    let system = |names: Option<&[&str]>| {
        names.is_some_and(|n| n.iter().any(|m| matches!(*m, "createdAt" | "modifiedAt")))
    };
    let sys = system(picked) || nested.clone().any(system);
    if let Some(names) = picked {
        // the computed values read their Attributes by term of the request's context
        let reads = filter
            .computed_reads
            .iter()
            .filter_map(|term| filter.context.get(term)?.as_str());
        let mut iris: Vec<&str> = Vec::new();
        for iri in asked(set, names)
            .map(|m| m.iri.as_str())
            .chain(expansions.iter().map(|e| e.iri))
            .chain(reads)
        {
            if !iris.contains(&iri) {
                iris.push(iri);
            }
        }
        // ponytail: an unpickable IRI reads the whole Entity, and a join then
        // walks every Relationship; mint terms (as `pushdown` does) if one is
        // ever seen
        if !iris.is_empty() && iris.iter().all(|iri| pickable(iri)) {
            let own = ["id", "type", "scope"].into_iter().chain(
                sys.then_some(["createdAt", "modifiedAt"])
                    .into_iter()
                    .flatten(),
            );
            let pick: Vec<String> = own.chain(iris).map(encode).collect();
            q.push_str("&pick=");
            q.push_str(&pick.join(","));
        }
    }
    if sys {
        q.push_str("&options=sysAttrs");
    }
    if joined {
        q.push_str("&join=inline&joinLevel=");
        q.push_str(&filter.linked.max(1).to_string());
    }
    q
}

/// The `$select` of each expansion of entities.
fn nested<'e>(expansions: &'e [Expansion]) -> impl Iterator<Item = Option<&'e [&'e str]>> + Clone {
    expansions.iter().filter_map(|e| match &e.writes {
        Writes::Entities(picked) => Some(picked.as_deref()),
        Writes::Refs(_) => None,
    })
}

/// Whether a read joins: an expansion of entities or a `q` that follows a
/// Relationship (5.7.2.4).
fn joins(expansions: &[Expansion], filter: &Translated) -> bool {
    filter.linked > 0 || nested(expansions).next().is_some()
}

/// The members a join without `$select` projects to: every one but the
/// navigations, so a navigation not expanded spends none of the walk's
/// lookups.
fn structural(set: &Set) -> Vec<&str> {
    set.members
        .iter()
        .filter(|(_, m)| !matches!(m.shape, Shape::Nav { .. }))
        .map(|(name, _)| name.as_str())
        .collect()
}

/// Which Entities a query selects: its type, `id`, `q`, `geoQ`,
/// `expandValues` and the `ngsild.*` parameters.
fn selection(set: &Set, filter: &Translated) -> String {
    let mut q = format!("type={}", encode(&set.type_iri));
    if let Some(ids) = &filter.ids {
        let ids: Vec<String> = ids.iter().map(|id| encode(id)).collect();
        q.push_str("&id=");
        q.push_str(&ids.join(","));
    }
    if let Some(text) = &filter.q {
        q.push_str("&q=");
        q.push_str(&encode(text));
    }
    if let Some((property, coordinates)) = &filter.geo {
        q.push_str("&georel=intersects&geometry=Polygon&coordinates=");
        q.push_str(&encode(coordinates));
        q.push_str("&geoproperty=");
        q.push_str(&encode(property));
    }
    if !filter.expand_values.is_empty() {
        q.push_str("&expandValues=");
        q.push_str(&encode(&filter.expand_values.join(",")));
    }
    q.push_str(&filter.ngsild);
    q
}

/// The pushdown of a read a residual or the surface sort completes
/// (ADR-0028 rule 1): [`selection`], the pushed `orderBy` keys, and the
/// selected Attributes, the expanded ones and those the sort reads, each by
/// a term of the request's own @context, so the scan tells apart what only
/// the residual reads. `ViewOps::scan` sets `limit`, `offset` and `count`.
pub(crate) fn pushdown(
    set: &Set,
    picked: Option<&[&str]>,
    expansions: &[Expansion],
    filter: &mut Translated,
) -> Pushdown {
    let joined = joins(expansions, filter);
    let frame: Vec<&str>;
    let picked = match picked {
        None if joined => {
            frame = structural(set);
            Some(frame.as_slice())
        }
        picked => picked,
    };
    let attrs = picked.map(|names| {
        let iris = asked(set, names)
            .map(|m| m.iri.as_str())
            .chain(expansions.iter().map(|e| e.iri))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut terms: Vec<String> = iris
            .iter()
            .map(|iri| mint(&mut filter.context, iri))
            .collect();
        for term in filter.sort_reads.iter().chain(&filter.computed_reads) {
            if !terms.contains(term) {
                terms.push(term.clone());
            }
        }
        terms
    });
    let system = |names: Option<&[&str]>| {
        names.is_some_and(|n| n.iter().any(|m| matches!(*m, "createdAt" | "modifiedAt")))
    };
    let mut query = selection(set, filter);
    if joined {
        query.push_str("&join=inline&joinLevel=");
        query.push_str(&filter.linked.max(1).to_string());
    }
    Pushdown {
        query,
        order: filter
            .order
            .as_deref()
            .map(|keys| keys.split(',').map(str::to_owned).collect())
            .unwrap_or_default(),
        attrs,
        sys_attrs: system(picked) || nested(expansions).any(system) || filter.sort_system,
        context: (!filter.context.is_empty()).then(|| Value::Object(filter.context.clone())),
    }
}

/// Where a collection read starts and how much it serves.
#[derive(Clone, Copy)]
struct Window {
    cursor: u64,
    skip: u64,
    top: Option<u64>,
    page: u64,
    counted: bool,
    version: ODataVersion,
}

/// A collection read the store answers whole: one Query Entities, offset
/// paging, the broker's count.
async fn pushed(
    reads: &Reads,
    caller: &Caller,
    (set, picked, expansions): (&Set, Option<&[&str]>, &[Expansion<'_>]),
    filter: &Translated,
    window: Window,
    link: &(dyn Fn(u64, u64, Option<u64>) -> String + Sync),
) -> Result<Served, ODataErrorResponse> {
    let Window {
        cursor,
        skip,
        top,
        page,
        counted,
        version,
    } = window;
    // Pushed whole: one Query Entities, offset paging. `$count` asks the broker's count;
    // without it one Entity past the page says whether a next page exists, so the store
    // counts nothing (T-5062: a count per page held 80 % of PostgreSQL's time at 1 000
    // distinct reads a second). A page at the broker's limit ceiling still counts.
    let probe = !counted && page < MAX_PAGE;
    let offset = cursor.saturating_add(skip);
    let target = format!(
        "/ngsi-ld/v1/entities?{}&limit={}&offset={offset}{}",
        query(set, picked, expansions, filter),
        if probe { page.saturating_add(1) } else { page },
        if probe { "" } else { "&count=true" },
    );
    let request = get(target, filter);
    let (answer, total) =
        answered(reads, caller, OperationId::QueryEntities, request, version).await?;
    let read = answer.as_deref().map_or(0, |b| {
        let read = Elements::<serde::de::IgnoredAny>::new(b).count();
        u64::try_from(read).unwrap_or(u64::MAX)
    });
    let past = probe && read > page;
    let served = read.min(page);
    let next = offset.saturating_add(served);
    let more = if probe {
        past && top.is_none_or(|top| served < top)
    } else {
        next < top.map_or(total, |top| total.min(offset.saturating_add(top)))
    };
    Ok(Served {
        docs: answer.map_or(Docs::Held(Vec::new()), |answer| {
            Docs::Answer(answer, usize::try_from(served).unwrap_or(usize::MAX))
        }),
        total,
        next: (served > 0 && more)
            .then(|| link(next, 0, top.map(|top| top.saturating_sub(served)))),
    })
}

/// A collection read the residual or the surface sort completes, through
/// `ViewOps::scan` (ADR-0028 rules 4 and 5).
async fn evaluated(
    reads: &Reads,
    caller: &Caller,
    (set, picked, expansions): (&Set, Option<&[&str]>, &[Expansion<'_>]),
    filter: &mut Translated,
    program: &Program,
    window: Window,
    link: &(dyn Fn(u64, u64, Option<u64>) -> String + Sync),
) -> Result<Served, ODataErrorResponse> {
    let Window {
        cursor,
        skip,
        top,
        page,
        counted,
        version,
    } = window;
    let pushdown = pushdown(set, picked, expansions, filter);
    Ok(if counted || filter.sort.is_some() {
        // The whole answer, counted and sorted here (ADR-0028 rule 5); the
        // cursor is a position in it. It is read in upstream parts and only what the page
        // can serve is kept (T-6049): the window of a count, the first `skip + top` of a
        // sort, the earlier in upstream order first among equals as a whole stable sort has
        // them.
        let window = (cursor, skip, top, page);
        let from = cursor.saturating_add(skip);
        let keep = from.saturating_add(top.unwrap_or(u64::MAX).min(page));
        let keep = usize::try_from(keep).unwrap_or(usize::MAX);
        let from = usize::try_from(from).unwrap_or(usize::MAX);
        let mut kept = Vec::new();
        let total = match &filter.sort {
            None => {
                let mut at = 0usize;
                scan_parts(reads, caller, &pushdown, program, version, |docs| {
                    for doc in docs {
                        if (from..keep).contains(&at) {
                            kept.push(doc);
                        }
                        at = at.saturating_add(1);
                    }
                    Ok(())
                })
                .await?
            }
            Some(keys) => {
                let total = scan_parts(reads, caller, &pushdown, program, version, |mut docs| {
                    attach(&mut docs, filter, version)?;
                    kept.append(&mut docs);
                    if kept.len() > keep.saturating_mul(2) {
                        sort(&mut kept, keys);
                        kept.truncate(keep);
                    }
                    Ok(())
                })
                .await?;
                sort(&mut kept, keys);
                kept.truncate(keep);
                kept.drain(..from.min(kept.len()));
                total
            }
        };
        let total = u64::try_from(total).unwrap_or(u64::MAX);
        let next = next_page(
            u64::try_from(kept.len()).unwrap_or(u64::MAX),
            total,
            window,
            link,
        );
        Served {
            docs: Docs::Held(kept),
            total,
            next,
        }
    } else {
        // A page filled from the upstream cursor (ADR-0028 rule 4); the
        // `$skip` is of the filtered answer, so it is dropped here and
        // what is left of it travels in the token.
        let fill = ScanPage {
            start: usize::try_from(cursor).unwrap_or(usize::MAX),
            size: usize::try_from(skip.saturating_add(page)).unwrap_or(usize::MAX),
            whole: false,
        };
        let (docs, next) = scanned(reads, caller, &pushdown, program, fill, version).await?;
        let dropped = docs.len().min(usize::try_from(skip).unwrap_or(usize::MAX));
        let skip = skip.saturating_sub(u64::try_from(dropped).unwrap_or(u64::MAX));
        let docs: Vec<Value> = docs.into_iter().skip(dropped).collect();
        let served = u64::try_from(docs.len()).unwrap_or(u64::MAX);
        let top = top.map(|top| top.saturating_sub(served));
        Served {
            docs: Docs::Held(docs),
            total: 0,
            next: next
                .filter(|_| top != Some(0))
                .map(|next| link(u64::try_from(next).unwrap_or(u64::MAX), skip, top)),
        }
    })
}

/// Where a collection read starts: the cursor, `$skip` and `$top` of the request or of its
/// token, and the page size.
pub(crate) fn position(
    reads: &Reads,
    options: &QueryOptions,
    digest: u64,
    headers: &HeaderMap,
    version: ODataVersion,
) -> Result<(u64, u64, Option<u64>, u64), ODataErrorResponse> {
    let (cursor, skip, top) = match &options.skiptoken {
        None => (0, options.skip.unwrap_or(0), options.top),
        // The token carries both: a $skip or $top beside it is an edit.
        Some(_) if options.skip.is_some() || options.top.is_some() => {
            return Err(bad_token(version))
        }
        Some(text) => read_token(text, digest).ok_or_else(|| bad_token(version))?,
    };
    let page = top
        .unwrap_or(u64::MAX)
        .min(max_page(headers, reads.state.max_header_len))
        .min(MAX_PAGE);
    Ok((cursor, skip, top, page))
}

/// The page of a whole answer `items` from `cursor` past `skip`, at most `top` in all and
/// `page` here: the items, the total, and the link to the next page.
pub(crate) fn paged<T>(
    items: Vec<T>,
    (cursor, skip, top, page): (u64, u64, Option<u64>, u64),
    link: &(dyn Fn(u64, u64, Option<u64>) -> String + Sync),
) -> (Vec<T>, u64, Option<String>) {
    let total = u64::try_from(items.len()).unwrap_or(u64::MAX);
    let from = cursor.saturating_add(skip);
    let end = top.map_or(total, |top| total.min(from.saturating_add(top)));
    let take = end.saturating_sub(from).min(page);
    let items: Vec<T> = items
        .into_iter()
        .skip(usize::try_from(from).unwrap_or(usize::MAX))
        .take(usize::try_from(take).unwrap_or(usize::MAX))
        .collect();
    let served = u64::try_from(items.len()).unwrap_or(u64::MAX);
    let next = next_page(served, total, (cursor, skip, top, page), link);
    (items, total, next)
}

/// The link after a page of `served` items of a whole answer of `total`, as [`paged`] serves
/// it.
fn next_page(
    served: u64,
    total: u64,
    (cursor, skip, top, _): (u64, u64, Option<u64>, u64),
    link: &(dyn Fn(u64, u64, Option<u64>) -> String + Sync),
) -> Option<String> {
    let from = cursor.saturating_add(skip);
    let end = top.map_or(total, |top| total.min(from.saturating_add(top)));
    let next = from.saturating_add(served);
    (served > 0 && next < end).then(|| link(next, 0, top.map(|top| top.saturating_sub(served))))
}

/// The page a collection read serves: its Entities, the count when asked,
/// and the token of the next page.
struct Served {
    docs: Docs,
    total: u64,
    next: Option<String>,
}

/// The Entities of a page: those a scan held, or the first of a store answer still in its
/// bytes, each parsed as it is written (T-6049: a page holds one parsed Entity at a time,
/// not the parsed answer beside its bytes and its converted copy).
enum Docs {
    Held(Vec<Value>),
    Answer(HeldAnswer, usize),
}

impl Docs {
    /// The Entities, each handed over once.
    fn each(&mut self) -> Box<dyn Iterator<Item = Value> + '_> {
        match self {
            Self::Held(docs) => Box::new(docs.drain(..)),
            Self::Answer(answer, served) => Box::new(Elements::new(answer).take(*served)),
        }
    }
}

/// The members of a JSON array (or the one document that is not an array), parsed one at
/// a time from its bytes; the first that does not parse ends them, as a broken answer read
/// whole parses to nothing.
pub(crate) struct Elements<'b, T> {
    bytes: &'b [u8],
    at: usize,
    of: std::marker::PhantomData<T>,
}

impl<'b, T> Elements<'b, T> {
    pub(crate) fn new(bytes: &'b [u8]) -> Self {
        let mut at = bytes
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(0);
        if bytes.get(at) == Some(&b'[') {
            at = at.saturating_add(1);
        }
        Self {
            bytes,
            at,
            of: std::marker::PhantomData,
        }
    }
}

impl<T: serde::de::DeserializeOwned> Iterator for Elements<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        let rest = self.bytes.get(self.at..).unwrap_or_default();
        let mut values = serde_json::Deserializer::from_slice(rest).into_iter::<T>();
        let value = values.next()?.ok()?;
        let after = rest.get(values.byte_offset()..).unwrap_or_default();
        let gap = after
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(after.len());
        let comma = usize::from(after.get(gap) == Some(&b','));
        self.at = self
            .at
            .saturating_add(values.byte_offset())
            .saturating_add(gap)
            .saturating_add(comma);
        Some(value)
    }
}

pub(crate) async fn collection(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    set_name: &str,
    path: &[Segment],
    options: &QueryOptions,
    headers: &HeaderMap,
) -> Result<Response, ODataErrorResponse> {
    let version = ctx.version;
    let set = set_of(reads, set_name, version)?;
    let expansions = expanded(&reads.edm, set, options, version)
        .map_err(|()| not_implemented("a nested $expand, $levels or a path in $expand", version))?;
    let mut filter = filtered(set, &reads.edm.sets, options, version)?;
    filter.annotate = annotated(headers);
    let picked = selected_of(set, options.select.as_ref(), &filter.computed).map_err(|()| {
        not_implemented("$select of a navigation, a path or an annotation", version)
    })?;
    let projection = ngsild(&reads.view, options, &mut filter, set, version)?;
    let picked = projected(set, picked, &projection);
    let digest = digest(&reads.state.service_root, path, options);
    let (cursor, skip, top, page) = position(reads, options, digest, headers, version)?;
    nulls_first(reads, caller, set, &mut filter, version).await?;
    if filter.sort.is_some() && filter.residual.is_empty() {
        // a surface sort reads the whole answer through the scan: a residual that always holds
        filter.residual.push(RExpr::And(Vec::new()));
    }
    let residual = residual_of(&filter, version)?;
    let counted = options.count == Some(true);
    let link = |cursor: u64, skip: u64, top: Option<u64>| {
        page_link(reads, path, options, token(cursor, skip, top, digest))
    };
    let window = Window {
        cursor,
        skip,
        top,
        page,
        counted,
        version,
    };
    let shape = (set, picked.as_deref(), expansions.as_slice());
    let served = match residual {
        None => pushed(reads, caller, shape, &filter, window, &link).await?,
        Some(program) => {
            evaluated(reads, caller, shape, &mut filter, &program, window, &link).await?
        }
    };
    let programs = programs_of(&filter, version)?;
    let Served {
        mut docs,
        total,
        next,
    } = served;
    let next_link = next;
    let names = listed(picked.as_deref(), &expansions, version);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let resource = resource(reads, set_name, version)?;
    let resource = if names.is_empty() {
        resource
    } else {
        resource.select(&names)
    };
    let page = Page {
        count: counted.then_some(total),
        next_link: next_link.as_deref(),
    };
    let mut body = Vec::new();
    // A Vec takes every byte: the writes cannot fail.
    if path.last() == Some(&Segment::Ref) {
        let mut ids = Vec::new();
        for mut doc in docs.each() {
            attach_one(&mut doc, &programs, version)?;
            let urn = doc.get("id").and_then(Value::as_str);
            ids.extend(urn.map(|urn| format!("{set_name}{}", key_segment(set, urn))));
        }
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let _ = write_entity_references(ctx, &ids, &page, &mut body);
        return Ok(json_response(version, ctx.level, body));
    }
    // Each Entity is converted and written in turn (T-6049): the page is never held as
    // parsed documents beside their converted copies.
    let _ = write_collection_start(ctx, &resource, page.count, &mut body);
    for (at, mut doc) in docs.each().enumerate() {
        attach_one(&mut doc, &programs, version)?;
        let entity = entity_value(
            reads,
            set_name,
            set,
            &doc,
            picked.as_deref(),
            &expansions,
            &filter,
        );
        let _ = write_collection_entity(ctx, &resource, &entity, at == 0, &mut body);
    }
    let _ = write_collection_end(ctx, page.next_link, &mut body);
    Ok(json_response(version, ctx.level, body))
}

pub(crate) async fn count(
    reads: &Reads,
    caller: &Caller,
    set_name: &str,
    options: &QueryOptions,
    version: ODataVersion,
) -> Result<Response, ODataErrorResponse> {
    let set = set_of(reads, set_name, version)?;
    if options.apply.is_some() {
        return Err(not_implemented("$apply under /$count", version));
    }
    let mut filter = filtered(set, &reads.edm.sets, options, version)?;
    ngsild(&reads.view, options, &mut filter, set, version)?;
    let total = match residual_of(&filter, version)? {
        None => {
            let target = format!(
                "/ngsi-ld/v1/entities?{}&limit=0&count=true",
                query(set, None, &[], &filter)
            );
            run(
                reads,
                caller,
                OperationId::QueryEntities,
                target,
                &filter,
                version,
            )
            .await?
            .1
        }
        Some(program) => {
            filter.sort = None;
            let pushdown = pushdown(set, None, &[], &mut filter);
            let total = scan_parts(reads, caller, &pushdown, &program, version, |_| Ok(())).await?;
            u64::try_from(total).unwrap_or(u64::MAX)
        }
    };
    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain")],
        total.to_string(),
    )
        .into_response())
}

pub(crate) async fn entity(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    set_name: &str,
    key: &Key,
    options: &QueryOptions,
    headers: &HeaderMap,
) -> Result<Response, ODataErrorResponse> {
    let version = ctx.version;
    let set = set_of(reads, set_name, version)?;
    if options.filter.is_some() || options.search.is_some() || options.apply.is_some() {
        return Err(not_implemented(
            "$filter, $search or $apply on one Entity",
            version,
        ));
    }
    // `$compute` alone: its values over the one Entity
    let computing = Translated {
        annotate: annotated(headers),
        ..Translated::default()
    };
    let computing = match &options.compute {
        Some(items) => computed_over(set, items, options, computing, version)?,
        None => computing,
    };
    let picked = selected_of(set, options.select.as_ref(), &computing.computed).map_err(|()| {
        not_implemented("$select of a navigation, a path or an annotation", version)
    })?;
    let expansions = expanded(&reads.edm, set, options, version)
        .map_err(|()| not_implemented("a nested $expand, $levels or a path in $expand", version))?;
    let id = admitted(reads, caller, set, key, options, version).await?;
    let target = format!(
        "/ngsi-ld/v1/entities/{}?{}",
        encode(&id),
        query(set, picked.as_deref(), &expansions, &computing),
    );
    let (mut docs, _) = run(
        reads,
        caller,
        OperationId::RetrieveEntity,
        target,
        &computing,
        version,
    )
    .await?;
    attach(&mut docs, &computing, version)?;
    let doc = docs.first().ok_or_else(|| not_found(version))?;
    let value = entity_value(
        reads,
        set_name,
        set,
        doc,
        picked.as_deref(),
        &expansions,
        &computing,
    );
    let names = listed(picked.as_deref(), &expansions, version);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let resource = resource(reads, set_name, version)?;
    let resource = if names.is_empty() {
        resource
    } else {
        resource.select(&names)
    };
    let mut body = Vec::new();
    // A Vec takes every byte: the write cannot fail.
    let _ = write_entity(ctx, &resource, &value, &mut body);
    Ok(json_response(version, ctx.level, body))
}

/// The request's `$filter` and `$search` as `q` and `id` (5.2) and its
/// `$orderby` as `orderBy` (Q9), empty without them.
fn filtered(
    set: &Set,
    sets: &std::collections::BTreeMap<String, Set>,
    options: &QueryOptions,
    version: ODataVersion,
) -> Result<Translated, ODataErrorResponse> {
    // `$search` is one more conjunct of the filter (Protocol 11.2.1: both before `$count`)
    let search = options.search.as_ref().and_then(|s| searched(s, set));
    let filter = match (options.filter.clone(), search) {
        (Some(f), Some(s)) => Some(Expr::And(Box::new(f), Box::new(s))),
        (f, s) => f.or(s),
    };
    let items = options.compute.as_deref().unwrap_or_default();
    // a filter on a computed value is one on what it computes (URL 5.1.10)
    let filter = filter.map(|f| inlined(&f, items));
    let lang = lang_of(options).map_err(|()| {
        ODataErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "BadRequest",
            "ngsild.lang is one language tag, such as sk or en-GB",
        )
        .with_version(version)
    })?;
    let translated = filter.as_ref().map_or_else(
        || Ok(Translated::default()),
        |f| translate(f, set, sets, &options.aliases, lang),
    );
    let translated = translated.map_err(|refused| refusal(refused, "$filter", version))?;
    let translated = computed_over(set, items, options, translated, version)?;
    match &options.orderby {
        Some(keys) => {
            let mut t = translated;
            order(keys, set, sets, &mut t)
                .map_err(|refused| refusal(refused, "$orderby", version))?;
            // each item pushed one key, in order
            t.held_keys = keys
                .iter()
                .zip(&t.pushed_keys)
                .filter_map(|(item, key)| match (&item.expr, &key.by) {
                    (Expr::Member(m), SortBy::Value(term))
                        if filter
                            .as_ref()
                            .zip(m.path.first())
                            .is_some_and(|(f, name)| holds_member(f, name)) =>
                    {
                        Some(term.clone())
                    }
                    _ => None,
                })
                .collect();
            Ok(t)
        }
        None => Ok(translated),
    }
}

/// `t` with the request's `$compute` `items` (Q16).
fn computed_over(
    set: &Set,
    items: &[odata_url::Compute],
    options: &QueryOptions,
    mut t: Translated,
    version: ODataVersion,
) -> Result<Translated, ODataErrorResponse> {
    compute(items, set, &options.aliases, &mut t, now())
        .map(|()| t)
        .map_err(|refused| refusal(refused, "$compute", version))
}

/// The name under which [`attach`] keeps an Entity's computed values: no
/// Attribute name starts with `@` (a JSON-LD keyword), so none collides.
pub(crate) const COMPUTED: &str = "@computed";

/// Each of `docs` with its computed values, by name, `null` for an unknown
/// one: computed over what the view and the engine let the Entity show, so
/// a hidden Attribute reads as an absent one. A document that holds them
/// already is left as it is.
pub(crate) fn attach(
    docs: &mut [Value],
    filter: &Translated,
    version: ODataVersion,
) -> Result<(), ODataErrorResponse> {
    let programs = programs_of(filter, version)?;
    for doc in docs {
        attach_one(doc, &programs, version)?;
    }
    Ok(())
}

/// The programs of the filter's computed values, `now` read once for a whole page.
fn programs_of(
    filter: &Translated,
    version: ODataVersion,
) -> Result<Vec<(String, Program)>, ODataErrorResponse> {
    if filter.computed.is_empty() {
        return Ok(Vec::new());
    }
    computed(filter, now()).map_err(|refused| refusal(refused, "$compute", version))
}

/// [`attach`] for one document.
fn attach_one(
    doc: &mut Value,
    programs: &[(String, Program)],
    version: ODataVersion,
) -> Result<(), ODataErrorResponse> {
    if programs.is_empty() || doc.get(COMPUTED).is_some() {
        return Ok(());
    }
    let mut values = serde_json::Map::new();
    for (name, program) in programs {
        let value = program
            .value(doc)
            .map_err(|_| refusal(Refused::TooComplex, "$compute", version))?;
        values.insert(name.clone(), value.unwrap_or(Value::Null));
    }
    // an Entity is a JSON object: the broker answers no other document
    let _ = doc
        .as_object_mut()
        .map(|members| members.insert(COMPUTED.to_owned(), Value::Object(values)));
    Ok(())
}

/// The OData error of a filter the surface does not take.
/// `option` names the query option the refused construct sits in.
pub(crate) fn refusal(refused: Refused, option: &str, version: ODataVersion) -> ODataErrorResponse {
    match refused {
        Refused::Later => not_implemented(&format!("this {option} expression"), version),
        Refused::Bad => ODataErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "BadRequest",
            "the query options cannot hold for this model",
        )
        .with_version(version),
        Refused::TooComplex => ODataErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "TooComplexQuery",
            "the filter is too complex",
        )
        .with_version(version),
    }
}

/// The digest a `$skiptoken` carries (S5.2): the view's service root and
/// the request without its paging, so a token read under another view or
/// another query does not match. ponytail: std `SipHash`; a forged token
/// does no more than `$skip`, every page is narrowed by the store again.
pub(crate) fn digest(root: &str, path: &[Segment], options: &QueryOptions) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let normal = ODataUri::Resource {
        path: path.to_vec(),
        options: QueryOptions {
            skip: None,
            top: None,
            skiptoken: None,
            ..options.clone()
        },
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{root}{normal}").hash(&mut hasher);
    hasher.finish()
}

/// `2.{cursor}.{$skip left}.{remaining $top or -}.{digest}` (Q20): the
/// cursor is the NGSI-LD offset, the upstream offset past the last Entity a
/// residual page served, or the position in a counted or sorted answer;
/// which one follows from the query the digest binds. A token of the form
/// before T-3758 does not read.
/// The URL of the page of the answer to `path` with `options` that `skiptoken` names.
pub(crate) fn page_link(
    reads: &Reads,
    path: &[Segment],
    options: &QueryOptions,
    skiptoken: String,
) -> String {
    let uri = ODataUri::Resource {
        path: path.to_vec(),
        options: QueryOptions {
            skip: None,
            top: None,
            skiptoken: Some(skiptoken),
            ..options.clone()
        },
    };
    format!("{}{uri}", reads.state.service_root)
}

pub(crate) fn token(cursor: u64, skip: u64, top: Option<u64>, digest: u64) -> String {
    let top = top.map_or_else(|| "-".to_owned(), |t| t.to_string());
    format!("2.{cursor}.{skip}.{top}.{digest:016x}")
}

/// The cursor, `$skip` left and remaining `$top` of a token issued for
/// `digest`.
fn read_token(text: &str, digest: u64) -> Option<(u64, u64, Option<u64>)> {
    let mut parts = text.split('.');
    let (Some("2"), Some(cursor), Some(skip), Some(top), Some(tag), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    if tag != format!("{digest:016x}") {
        return None;
    }
    let top = match top {
        "-" => None,
        n => Some(n.parse().ok()?),
    };
    Some((cursor.parse().ok()?, skip.parse().ok()?, top))
}

fn bad_token(version: ODataVersion) -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::BAD_REQUEST,
        "BadRequest",
        "the skip token does not belong to this request",
    )
    .with_version(version)
}

/// The Entity id a key predicate names, either through a parameter alias:
/// `('urn:…')` or `(id='urn:…')` (E2); on a set with an identifier (E2a)
/// its value `(v)` or `(ID=v)` names `{prefix}{v}`, and `('urn:…')` stays
/// a URN where the identifier is no string.
pub(crate) fn key_id(set: &Set, key: &Key, options: &QueryOptions) -> Option<String> {
    let own = set.identifier.as_ref();
    let (value, by_id) = match key {
        Key::Single(v) => (v, false),
        Key::Compound(pairs) => pairs
            .iter()
            .find(|(name, _)| name == "id")
            .map(|(_, v)| (v, true))
            .or_else(|| {
                let i = own?;
                pairs
                    .iter()
                    .find(|(name, _)| *name == i.member)
                    .map(|(_, v)| (v, false))
            })?,
    };
    let literal = match value {
        KeyValue::Literal(l) => l,
        KeyValue::Alias(name) => &options.aliases.iter().find(|(n, _)| n == name)?.1,
    };
    match (own.filter(|_| !by_id), literal) {
        (Some(i), Literal::Guid(g)) if i.edm == "Edm.Guid" => Some(format!("{}{g}", i.prefix)),
        (Some(i), Literal::Int(n)) if integer(i.edm) => Some(format!("{}{n}", i.prefix)),
        (Some(i), Literal::String(s)) if i.edm == "Edm.String" => Some(format!("{}{s}", i.prefix)),
        (_, Literal::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// The id `key` names in `set`, as [`key_id`] reads it; in an output's set (T-5225) only when
/// the output's filter selects that Entity: one Query Entities of the id under the filter, the
/// 404 of an absent one otherwise.
pub(crate) async fn admitted(
    reads: &Reads,
    caller: &Caller,
    set: &Set,
    key: &Key,
    options: &QueryOptions,
    version: ODataVersion,
) -> Result<String, ODataErrorResponse> {
    let id = key_id(set, key, options).ok_or_else(|| not_found(version))?;
    if set.output.is_none() {
        return Ok(id);
    }
    let mut filter = Translated {
        ids: Some(vec![id.clone()]),
        ..Translated::default()
    };
    ngsild(
        &reads.view,
        &QueryOptions::default(),
        &mut filter,
        set,
        version,
    )?;
    let target = format!("/ngsi-ld/v1/entities?{}&limit=1", selection(set, &filter));
    let (docs, _) = run(
        reads,
        caller,
        OperationId::QueryEntities,
        target,
        &filter,
        version,
    )
    .await?;
    if docs.is_empty() {
        return Err(not_found(version));
    }
    Ok(id)
}

/// The canonical key predicate of Entity `urn` in `set`: its identifier's
/// value when the URN follows `{prefix}{v}` and `v` reads back as the
/// same URN (E2a), else `(id='urn:…')`; `('urn:…')` without an identifier.
pub(crate) fn key_segment(set: &Set, urn: &str) -> Segment {
    Segment::Key(canonical_key(set, urn))
}

/// The key of [`key_segment`].
pub(crate) fn canonical_key(set: &Set, urn: &str) -> Key {
    let string = |s: &str| KeyValue::Literal(Literal::String(s.to_owned()));
    let Some(own) = &set.identifier else {
        return Key::Single(string(urn));
    };
    let value = urn
        .strip_prefix(own.prefix.as_str())
        .and_then(|v| match own.edm {
            "Edm.Guid" => is_guid(v).then(|| Literal::Guid(v.to_owned())),
            edm if integer(edm) => v
                .parse()
                .ok()
                .filter(|n: &i64| n.to_string() == v)
                .map(Literal::Int),
            _ => Some(Literal::String(v.to_owned())),
        });
    match value {
        Some(v) => Key::Single(KeyValue::Literal(v)),
        None => Key::Compound(vec![("id".into(), string(urn))]),
    }
}

/// One of the integer Edm types a key may have (T-3722).
fn integer(edm: &str) -> bool {
    matches!(
        edm,
        "Edm.Byte" | "Edm.SByte" | "Edm.Int16" | "Edm.Int32" | "Edm.Int64"
    )
}

/// `8-4-4-4-12` lower-case hex digits, as a `guid` literal reads back.
fn is_guid(v: &str) -> bool {
    v.len() == 36
        && v.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => matches!(c, '0'..='9' | 'a'..='f'),
        })
}

/// The Entity id a key member of a write body names: the URN itself (E2),
/// or the identifier's value as in [`key_id`] (E2a).
pub(crate) fn body_id(set: &Set, value: &Value) -> Option<String> {
    match (&set.identifier, value) {
        (None, Value::String(s)) => Some(s.clone()),
        (Some(i), Value::String(s)) if i.edm == "Edm.Guid" => {
            Some(format!("{}{}", i.prefix, s.to_ascii_lowercase()))
        }
        (Some(i), Value::String(s)) => Some(format!("{}{s}", i.prefix)),
        (Some(i), Value::Number(n)) => Some(format!("{}{n}", i.prefix)),
        _ => None,
    }
}

/// Runs one operation; the Entities of its answer and the result count.
pub(crate) async fn run(
    reads: &Reads,
    caller: &Caller,
    op: OperationId,
    target: String,
    filter: &Translated,
    version: ODataVersion,
) -> Result<(Vec<Value>, u64), ODataErrorResponse> {
    run_request(reads, caller, op, get(target, filter), version).await
}

/// A GET of `target` under the filter's terms.
fn get(target: String, filter: &Translated) -> OpsRequest {
    let req = OpsRequest::new(Method::GET, target);
    if filter.context.is_empty() {
        req
    } else {
        req.context(Value::Object(filter.context.clone()))
    }
}

/// Runs one operation of any method; the Entities of its answer, if it
/// has a body, and the result count.
pub(crate) async fn run_request(
    reads: &Reads,
    caller: &Caller,
    op: OperationId,
    req: OpsRequest,
    version: ODataVersion,
) -> Result<(Vec<Value>, u64), ODataErrorResponse> {
    let (answer, total) = answered(reads, caller, op, req, version).await?;
    let docs = answer.map_or_else(Vec::new, |bytes| Elements::new(&bytes).collect());
    Ok((docs, total))
}

/// [`run_request`] with the answer left as its bytes, `None` for the empty one a Tenant
/// that holds nothing answers.
async fn answered(
    reads: &Reads,
    caller: &Caller,
    op: OperationId,
    req: OpsRequest,
    version: ODataVersion,
) -> Result<(Option<HeldAnswer>, u64), ODataErrorResponse> {
    let answer = reads
        .view
        .run(op, caller, req)
        .await
        .map_err(|refusal: Refusal| refused(refusal.status(), version))?;
    if answer.status() == StatusCode::NOT_FOUND {
        // A query answers 404 for a Tenant that holds nothing yet (5.5.10);
        // its EntitySet is empty.
        return match op {
            OperationId::QueryEntities | OperationId::TemporalQuery => Ok((None, 0)),
            _ => Err(not_found(version)),
        };
    }
    if answer.status() == StatusCode::BAD_REQUEST {
        return Err(model_refusal(reads, answer, version).await);
    }
    if !answer.status().is_success() {
        return Err(refused(answer.status(), version));
    }
    let truncated = answer
        .headers()
        .get_all("NGSILD-Warning")
        .iter()
        .any(|w| w.as_bytes().windows(9).any(|at| at == b"truncated"));
    if truncated {
        return Err(truncated_join(version));
    }
    let total = answer
        .headers()
        .get("NGSILD-Results-Count")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let bytes = reads
        .view
        .read_answer(answer.into_body(), "odata")
        .await
        .map_err(|refusal| refused(refusal.status(), version))?;
    Ok((Some(bytes), total))
}

/// One NGSI-LD Entity, as normalized JSON keyed by the core context and the
/// filter's terms, as the members of `set` (or the `picked` ones) and the
/// expanded navigation members.
pub(crate) fn entity_value(
    reads: &Reads,
    set_name: &str,
    set: &Set,
    doc: &Value,
    picked: Option<&[&str]>,
    expansions: &[Expansion],
    filter: &Translated,
) -> EntityValue {
    let id = doc.get("id").and_then(Value::as_str).unwrap_or_default();
    // A selection keeps the key (Protocol 11.2.5.1): the E2a identifier, else `id`.
    let key = key_member(set);
    let wanted = |name: &str| picked.is_none_or(|p| p.contains(&name)) || key == Some(name);
    let mut properties = Vec::new();
    if wanted("id") || key.is_none() {
        properties.push(("id".to_owned(), Value::from(id)));
    }
    if wanted("_types") {
        properties.push(("_types".into(), Value::from(visible_types(&reads.edm, doc))));
    }
    for system in ["scope", "createdAt", "modifiedAt"] {
        // The temporal members only when named (E6): a nested $select may
        // have asked the broker for them.
        let named = system == "scope" || picked.is_some_and(|p| p.contains(&system));
        if let Some(value) = doc.get(system).filter(|_| wanted(system) && named) {
            let value = match (system, value) {
                ("scope", Value::String(_)) => Value::Array(vec![value.clone()]),
                _ => value.clone(),
            };
            properties.push((system.into(), value));
        }
    }
    for (name, member) in &set.members {
        if !wanted(name) || matches!(member.shape, Shape::Nav { .. }) {
            continue;
        }
        let instance = default_instance(doc.get(key_of(&member.iri, filter)));
        let (value, unserved) = served(&member.shape, instance);
        // Instance annotations precede the property they annotate (JSON Format 4.01 20).
        if let Some(why) = unserved {
            properties.push((format!("{name}@{VOCABULARY}.Unserved"), Value::from(why)));
        }
        if filter.annotate {
            for (key, term) in [("observedAt", "observedAt"), ("unitCode", "unitCode")] {
                if let Some(v) = instance.and_then(|i| i.get(key)) {
                    properties.push((format!("{name}@{VOCABULARY}.{term}"), v.clone()));
                }
            }
        }
        properties.push((name.clone(), value));
    }
    for e in expansions {
        let instance = default_instance(doc.get(key_of(e.iri, filter)));
        properties.push((e.name.to_owned(), expansion(reads, e, instance, filter)));
    }
    for (name, _) in &filter.computed {
        if wanted(name) {
            let value = doc.get(COMPUTED).and_then(|c| c.get(name));
            properties.push((name.clone(), value.cloned().unwrap_or(Value::Null)));
        }
    }
    EntityValue {
        id: format!("{set_name}{}", key_segment(set, id)),
        properties,
    }
}

/// The member that keys `set` (E2a), `None` when the key is `id`.
fn key_member(set: &Set) -> Option<&str> {
    set.identifier.as_ref().map(|i| i.member.as_str())
}

/// The members a read of `names` asks the broker for: those, and the key a selection keeps.
fn asked<'s>(set: &'s Set, names: &'s [&str]) -> impl Iterator<Item = &'s crate::edm::Member> {
    names
        .iter()
        .copied()
        .chain(key_member(set).filter(|key| !names.contains(key)))
        .filter_map(|name| set.members.get(name))
}

/// The key of an Attribute in an answer compacted under the request's
/// context: a term the filter defined, else the core name.
fn key_of(iri: &str, filter: &Translated) -> String {
    filter
        .context
        .iter()
        .find(|(_, defined)| defined.as_str() == Some(iri))
        .map_or_else(|| core_name(iri), |(term, _)| term.clone())
}

/// An expanded navigation member: references to the Relationship's
/// objects, or the targets the join embedded. A target the view or the
/// policy hides is not embedded (T-3470), nor one of another type: it reads
/// like an absent target, `null` or left out of the collection (O5).
fn expansion(reads: &Reads, e: &Expansion, instance: Option<&Value>, filter: &Translated) -> Value {
    let found: Vec<Value> = match &e.writes {
        Writes::Refs(id) => objects(instance)
            .into_iter()
            .map(|urn| {
                let key = key_segment(e.target, &urn);
                serde_json::json!({ *id: format!("{}{key}", e.target_name) })
            })
            .collect(),
        Writes::Entities(picked) => {
            let wanted = core_name(&e.target.type_iri);
            ["entity", "entityList"]
                .iter()
                .filter_map(|m| instance.and_then(|i| i.get(*m)))
                .flat_map(|v| match v {
                    Value::Array(docs) => docs.clone(),
                    one => vec![one.clone()],
                })
                .filter(|doc| types_of(doc).contains(&wanted.as_str()))
                .map(|doc| {
                    let value = entity_value(
                        reads,
                        e.target_name,
                        e.target,
                        &doc,
                        picked.as_deref(),
                        &[],
                        filter,
                    );
                    Value::Object(value.properties.into_iter().collect())
                })
                .collect()
        }
    };
    if e.collection {
        Value::Array(found)
    } else {
        found.into_iter().next().unwrap_or(Value::Null)
    }
}

/// The object URIs of a Relationship instance: `object` (one or many) or a
/// ListRelationship's `objectList`.
pub(crate) fn objects(instance: Option<&Value>) -> Vec<String> {
    let Some(instance) = instance else {
        return Vec::new();
    };
    let items = |v: &Value| match v {
        Value::Array(items) => items.clone(),
        one => vec![one.clone()],
    };
    instance
        .get("object")
        .map(items)
        .into_iter()
        .chain(instance.get("objectList").map(items))
        .flatten()
        .filter_map(|v| match v {
            Value::String(urn) => Some(urn),
            Value::Object(o) => o.get("object").and_then(Value::as_str).map(str::to_owned),
            _ => None,
        })
        .collect()
}

/// An Entity's types as its answer names them.
fn types_of(doc: &Value) -> Vec<&str> {
    match doc.get("type") {
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        Some(one) => one.as_str().into_iter().collect(),
        None => Vec::new(),
    }
}

/// The sets of the Entity's types the model holds, by name.
fn visible_types(edm: &Edm, doc: &Value) -> Vec<String> {
    let types = types_of(doc);
    edm.sets
        .iter()
        .filter(|(_, s)| s.output.is_none() && types.contains(&core_name(&s.type_iri).as_str()))
        .map(|(name, _)| name.clone())
        .collect()
}

/// The default instance, the one without a `datasetId` (S3, TS 104 175 5.2.6.4.1); `None` when
/// every instance is named: a named instance is never served as the member (T-5206 finding 8).
pub(crate) fn default_instance(attr: Option<&Value>) -> Option<&Value> {
    match attr? {
        Value::Array(instances) => instances.iter().find(|i| i.get("datasetId").is_none()),
        one => one.get("datasetId").is_none().then_some(one),
    }
}

/// The OData value of one Attribute instance (rows P1 to P7, R5).
pub(crate) fn member_value(shape: &Shape, instance: Option<&Value>) -> Value {
    served(shape, instance).0
}

/// [`member_value`] and, when the stored instance holds what the member cannot serve, why it
/// reads `null` (T-5206 findings 7 and 13): an array under a single-valued member, an instance
/// of another kind than the model names, a LanguageProperty without the text asked for.
pub(crate) fn served(shape: &Shape, instance: Option<&Value>) -> (Value, Option<&'static str>) {
    let member = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| instance.and_then(|i| i.get(*n)))
            .cloned()
    };
    let value = match shape {
        Shape::Value { .. } => member(&["value", "valueList"]).map(plain),
        Shape::Geo | Shape::Json => member(&["value", "json"]),
        Shape::Language => member(&["value"])
            .or_else(|| member(&["languageMap"]).and_then(|m| m.get("@none").cloned())),
        Shape::Vocab => member(&["vocab"]),
        Shape::Url { .. } | Shape::Nav { .. } => member(&["object", "objectList"]),
    };
    let collection = matches!(
        shape,
        Shape::Value {
            collection: true,
            ..
        } | Shape::Url { collection: true }
    );
    let other_kind = instance.is_some_and(Value::is_object) && value.is_none();
    let value = match (shape, value) {
        (Shape::Value { edm, .. }, Some(v)) => match as_edm(edm, v) {
            Ok(v) => Some(v),
            Err(note) => return (Value::Null, Some(note)),
        },
        (_, v) => v,
    };
    match value {
        // An Untyped member holds any JSON value, an array too.
        Some(v) if *shape == Shape::Json => (v, None),
        Some(Value::Array(items)) if !collection && items.is_empty() => (Value::Null, None),
        Some(Value::Array(_)) if !collection => (
            Value::Null,
            Some("the stored value is an array; this member holds one value"),
        ),
        Some(one) if collection && !one.is_array() => (Value::Array(vec![one]), None),
        Some(v) => (v, None),
        None if collection => (Value::Array(Vec::new()), other_kind.then_some(KIND_NOTE)),
        None if *shape == Shape::Language && other_kind => (
            Value::Null,
            Some("the LanguageProperty has no text without a language: ask with ngsild.lang"),
        ),
        None => (Value::Null, other_kind.then_some(KIND_NOTE)),
    }
}

/// A stored value in the JSON form of `edm` (JSON Format 7.1), item by item in a list: an
/// `Edm.Binary` as unpadded base64url whichever base64 NGSI-LD stores, an `Edm.TimeOfDay`
/// without the `Z` NGSI-LD writes a Time with (TS 104 175 5.2.2.4). `Err` with the reason when
/// a stored text is no binary value (T-5905).
fn as_edm(edm: &str, value: Value) -> Result<Value, &'static str> {
    match (edm, value) {
        (_, Value::Array(items)) => items
            .into_iter()
            .map(|v| as_edm(edm, v))
            .collect::<Result<_, _>>()
            .map(Value::Array),
        ("Edm.Binary", Value::String(text)) => decode_base64(&text)
            .map(|bytes| Value::String(base64url(&bytes)))
            .ok_or("the stored value is no base64 text"),
        ("Edm.TimeOfDay", Value::String(text)) => Ok(Value::String(
            text.strip_suffix(['Z', 'z'])
                .map_or(text.clone(), str::to_owned),
        )),
        (_, v) => Ok(v),
    }
}

/// Why a stored instance of another kind than the model names reads `null`.
const KIND_NOTE: &str = "the stored Attribute is of another kind than the model names";

/// A typed literal (`{"@type": "DateTime", "@value": …}`) as its value,
/// item by item in a list.
fn plain(value: Value) -> Value {
    match value {
        Value::Object(mut o) if o.contains_key("@value") => {
            o.remove("@value").unwrap_or(Value::Null)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(plain).collect()),
        other => other,
    }
}

/// `ngsild.lang`: the language a LanguageProperty is read and filtered in (NGSI-LD `lang`,
/// TS 104 175 4.15), a BCP 47 tag of letters, digits and `-`; `Err` for any other value.
fn lang_of(options: &QueryOptions) -> Result<Option<&str>, ()> {
    let Some((_, value)) = options.custom.iter().find(|(n, _)| n == "ngsild.lang") else {
        return Ok(None);
    };
    match value.as_deref() {
        Some(tag)
            if (1..=35).contains(&tag.len())
                && tag.starts_with(|c: char| c.is_ascii_alphabetic())
                && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') =>
        {
            Ok(Some(tag))
        }
        _ => Err(()),
    }
}

/// The NGSI-LD options beside the query (ADR-0028 rule 6, Q27), applied to
/// `filter`: `ngsild.q` joined to its `q` by `;`, `ngsild.scopeQ` and the geo
/// options pushed as written, a pushed `geo.intersects` moved to the
/// residual when they take the one `geoQ`. Each name is read under the
/// view's `@context` and written as a term of the request's own. What
/// `ngsild.pick` and `ngsild.omit` name, as IRIs, is returned.
pub(crate) fn ngsild(
    view: &ViewOps,
    options: &QueryOptions,
    filter: &mut Translated,
    set: &Set,
    version: ODataVersion,
) -> Result<Projection, ODataErrorResponse> {
    let bad = |message: &str| {
        ODataErrorResponse::new(StatusCode::BAD_REQUEST, "BadRequest", message)
            .with_version(version)
    };
    // an output's set (T-5225): its filter's `q` a conjunct of the request's, its `scopeQ`
    // joined with the request's `ngsild.scopeQ` below
    let ours = set.output.as_ref();
    if let Some(q) = ours.and_then(|o| o.q.as_deref()) {
        let q = view
            .rename_q(q, &mut |iri| mint(&mut filter.context, iri))
            .map_err(|_| bad("this EntitySet's filter is not an NGSI-LD query"))?;
        filter.q = Some(match filter.q.take() {
            Some(theirs) => format!("({q});({theirs})"),
            None => format!("({q})"),
        });
    }
    let theirs_scope = options
        .custom
        .iter()
        .find(|(name, _)| name == "ngsild.scopeQ")
        .and_then(|(_, v)| v.as_deref());
    if let Some(o) = ours {
        let theirs = theirs_scope
            .map(|v| {
                antares_ops::percent::decoded(v)
                    .ok_or_else(|| bad("ngsild.scopeQ is not percent-encoded"))
            })
            .transpose()?;
        if let Some(scope) = o.scope_with(theirs.as_deref()).map_err(bad)? {
            filter.ngsild.push_str("&scopeQ=");
            filter.ngsild.push_str(&encode(&scope));
        }
    }
    let mut projection = Projection::default();
    let mut geo = false;
    for (i, (name, value)) in options.custom.iter().enumerate() {
        if options
            .custom
            .iter()
            .take(i)
            .any(|(earlier, _)| earlier == name)
        {
            return Err(bad("an ngsild option is given twice"));
        }
        let value = value
            .as_deref()
            .ok_or_else(|| bad("an ngsild option has no value"))?;
        let names = |value| {
            view.expand_names(value)
                .map_err(|_| bad("an ngsild option names no Attributes"))
        };
        match name.as_str() {
            "ngsild.q" => {
                let theirs = view
                    .rename_q(value, &mut |iri| mint(&mut filter.context, iri))
                    .map_err(|_| bad("ngsild.q is not an NGSI-LD query"))?;
                filter.q = Some(match filter.q.take() {
                    Some(ours) => format!("{ours};({theirs})"),
                    None => format!("({theirs})"),
                });
            }
            "ngsild.geoproperty" => {
                let [iri] = names(value)?
                    .try_into()
                    .map_err(|_| bad("ngsild.geoproperty names one Attribute"))?;
                let term = mint(&mut filter.context, &iri);
                filter.ngsild.push_str("&geoproperty=");
                filter.ngsild.push_str(&encode(&term));
                geo = true;
            }
            "ngsild.scopeQ" if ours.is_some() => {}
            // checked by `lang_of`: the broker serves each LanguageProperty in it
            "ngsild.lang" => {
                filter.ngsild.push_str("&lang=");
                filter.ngsild.push_str(value);
            }
            "ngsild.pick" => projection.pick = Some(names(value)?),
            "ngsild.omit" => projection.omit = Some(names(value)?),
            // the value is a URL's, percent-encoded as written: one parameter
            other => {
                let parameter = other.trim_start_matches("ngsild.");
                geo |= parameter != "scopeQ";
                filter.ngsild.push('&');
                filter.ngsild.push_str(parameter);
                filter.ngsild.push('=');
                filter.ngsild.push_str(value);
            }
        }
    }
    if geo && filter.geo.take().is_some() {
        // the request's one geoQ is the caller's; the OData conjunct is evaluated
        filter.residual.extend(filter.geo_residual.take());
    }
    Ok(projection)
}

/// What `ngsild.pick` keeps and `ngsild.omit` drops, as IRIs.
#[derive(Default)]
pub(crate) struct Projection {
    pick: Option<Vec<String>>,
    omit: Option<Vec<String>>,
}

/// The members a read writes, narrowed by `ngsild.pick` and `ngsild.omit`
/// (TS 104 175 4.21: projection after selection): an Attribute stays when
/// the pick names it and the omit does not; the base members stay as
/// selected.
fn projected<'s>(
    set: &'s Set,
    picked: Option<Vec<&'s str>>,
    projection: &Projection,
) -> Option<Vec<&'s str>> {
    if projection.pick.is_none() && projection.omit.is_none() {
        return picked;
    }
    let names = picked.unwrap_or_else(|| {
        ["_types", "scope"]
            .into_iter()
            .chain(
                set.members
                    .iter()
                    .filter(|(_, m)| !matches!(m.shape, Shape::Nav { .. }))
                    .map(|(name, _)| name.as_str()),
            )
            .collect()
    });
    let holds =
        |list: &Option<Vec<String>>, iri: &str| list.as_ref().map(|l| l.iter().any(|n| n == iri));
    Some(
        names
            .into_iter()
            .filter(|name| match set.members.get(*name) {
                Some(m) => {
                    holds(&projection.pick, &m.iri) != Some(false)
                        && holds(&projection.omit, &m.iri) != Some(true)
                }
                None => true,
            })
            .collect(),
    )
}

/// URL 5.1.5 puts null first ascending and last descending; the store puts
/// an Entity lacking an `orderBy` key last ascending and first descending.
/// One count of the Entities lacking a key decides: none, and the store's
/// order is the standard's; any, and the read is sorted on the surface
/// (T-3853). A key the `$filter` holds on every Entity it admits needs no
/// count, a second scan of the match set (T-4919).
// ponytail: a sparse key over RESIDUAL_SCAN Entities is 400 TooManyResults;
// split a single-key sort into "lacks it" and "holds it" reads if that matters
async fn nulls_first(
    reads: &Reads,
    caller: &Caller,
    set: &Set,
    filter: &mut Translated,
    version: ODataVersion,
) -> Result<(), ODataErrorResponse> {
    let terms: Vec<&str> = filter
        .pushed_keys
        .iter()
        .filter_map(|k| match &k.by {
            SortBy::Value(term) => Some(term.as_str()),
            SortBy::Id | SortBy::System(_) | SortBy::Computed(_) | SortBy::Linked(..) => None,
        })
        .collect();
    let unheld: Vec<&&str> = terms
        .iter()
        .filter(|t| !filter.held_keys.iter().any(|h| h == **t))
        .collect();
    if filter.order.is_none() || unheld.is_empty() {
        return Ok(());
    }
    let lacking = unheld
        .iter()
        .map(|t| format!("!{t}"))
        .collect::<Vec<_>>()
        .join("|");
    let q = match &filter.q {
        Some(q) => format!("({q});({lacking})"),
        None => lacking,
    };
    let probe = Translated {
        q: Some(q),
        ..filter.probe()
    };
    let target = format!(
        "/ngsi-ld/v1/entities?{}&limit=0&count=true",
        query(set, None, &[], &probe)
    );
    let (_, lacking) = run(
        reads,
        caller,
        OperationId::QueryEntities,
        target,
        &probe,
        version,
    )
    .await?;
    if lacking > 0 {
        filter.sort_reads = terms.iter().map(|t| (*t).to_owned()).collect();
        filter.sort = Some(std::mem::take(&mut filter.pushed_keys));
        filter.order = None;
    }
    Ok(())
}

/// The request's residual as a program, `now` read once (ADR-0028 rule 2).
fn residual_of(
    filter: &Translated,
    version: ODataVersion,
) -> Result<Option<Program>, ODataErrorResponse> {
    program(filter, now()).map_err(|refused| refusal(refused, "$filter", version))
}

/// The wall clock; one before 1970 reads as 1970, one past 9999 as the last instant.
pub(crate) fn now() -> DateTime {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros());
    i64::try_from(micros)
        .ok()
        .and_then(DateTime::from_unix_micros)
        .unwrap_or(DateTime::MAX)
}

/// One `ViewOps::scan` of `program`: the Entities it served and the
/// upstream offset to continue from.
pub(crate) async fn scanned(
    reads: &Reads,
    caller: &Caller,
    pushdown: &Pushdown,
    program: &Program,
    page: ScanPage,
    version: ODataVersion,
) -> Result<(Vec<Value>, Option<usize>), ODataErrorResponse> {
    let result = reads.view.scan(caller, pushdown, Some(program), page).await;
    scan_answer(result, version)
}

/// A whole scanned answer read in upstream parts of at most one store read of admitted
/// Entities each (T-6049): `take` is handed each part in upstream order, and the count of all
/// is returned. The parts share one budget of upstream reads: an answer past
/// [`RESIDUAL_SCAN`] is refused as a whole read of it is.
async fn scan_parts(
    reads: &Reads,
    caller: &Caller,
    pushdown: &Pushdown,
    program: &Program,
    version: ODataVersion,
    mut take: impl FnMut(Vec<Value>) -> Result<(), ODataErrorResponse>,
) -> Result<usize, ODataErrorResponse> {
    let mut budget = RESIDUAL_SCAN.saturating_add(1);
    let (mut start, mut count) = (0, 0usize);
    loop {
        let page = ScanPage {
            start,
            size: reads.view.page_size(),
            whole: false,
        };
        let result = (reads.view)
            .scan_within(caller, pushdown, program, page, &mut budget)
            .await;
        let (docs, next) = scan_answer(result, version)?;
        count = count.saturating_add(docs.len());
        take(docs)?;
        match next {
            None => return Ok(count),
            Some(_) if budget == 0 => return Err(too_many(version)),
            Some(next) => start = next,
        }
    }
}

/// The OData answer of one scan.
fn scan_answer(
    result: Result<Scanned, Refusal>,
    version: ODataVersion,
) -> Result<(Vec<Value>, Option<usize>), ODataErrorResponse> {
    match result {
        // T-3823: a scanned join is refused like a pushed one when the walk stopped short
        Ok(Scanned::Page {
            truncated: true, ..
        }) => Err(truncated_join(version)),
        Ok(Scanned::Page { entities, next, .. }) => Ok((entities, next)),
        // A query answers 404 for a Tenant that holds nothing yet (5.5.10).
        Ok(Scanned::Answer(answer)) if answer.status() == StatusCode::NOT_FOUND => {
            Ok((Vec::new(), None))
        }
        Ok(Scanned::Answer(answer)) => Err(refused(answer.status(), version)),
        Err(Refusal::TooManyResults { .. }) => Err(too_many(version)),
        // An evaluation past the residual's MAX_STEPS: a nested `any` over a long list reaches
        // it within MAX_NODES (T-4031)
        Err(Refusal::Malformed(TOO_COMPLEX)) => {
            Err(
                ODataErrorResponse::new(StatusCode::BAD_REQUEST, "TooComplexQuery", TOO_COMPLEX)
                    .with_version(version),
            )
        }
        Err(other) => Err(refused(other.status(), version)),
    }
}

/// The join walk stopped at the broker's lookup budget (4.5.23.1): an OData
/// client cannot see the NGSI-LD warning, so a partial expansion is refused
/// (S1, O6, D9).
fn too_many(version: ODataVersion) -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::BAD_REQUEST,
        "TooManyResults",
        &format!(
            "the answer holds more than the {RESIDUAL_SCAN} Entities one request may read: narrow it with $filter"
        ),
    )
    .with_version(version)
}

fn truncated_join(version: ODataVersion) -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::BAD_REQUEST,
        "BadRequest",
        "the expansion reaches more Entities than one answer embeds: narrow it with $top or $filter",
    )
    .with_version(version)
}

/// `docs` in the order of `keys` (Q9, URL 5.1.5): null and missing values
/// first ascending, numbers by value, strings by code point; ties keep the
/// upstream order, which ends in `id`.
fn sort(docs: &mut [Value], keys: &[SortKey]) {
    docs.sort_by(|a, b| {
        keys.iter()
            .map(|k| {
                let o = compare(sort_value(a, &k.by), sort_value(b, &k.by));
                if k.descending {
                    o.reverse()
                } else {
                    o
                }
            })
            .find(|o| o.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// The value a sort key reads: a JSON-LD value object by its `@value`.
fn sort_value<'d>(doc: &'d Value, by: &SortBy) -> Option<&'d Value> {
    let found = match by {
        SortBy::Id => doc.get("id"),
        SortBy::System(name) => doc.get(name),
        SortBy::Value(term) => default_instance(doc.get(term)).and_then(|i| i.get("value")),
        SortBy::Computed(name) => doc.get(COMPUTED).and_then(|c| c.get(name)),
        SortBy::Linked(rel, term) => default_instance(doc.get(rel))
            .and_then(|i| i.get("entity"))
            .and_then(|target| default_instance(target.get(term)))
            .and_then(|i| i.get("value")),
    };
    found
        .map(|v| v.get("@value").unwrap_or(v))
        .filter(|v| !v.is_null())
}

pub(crate) fn compare(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let kind = |v: &Value| match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    };
    match (a, b) {
        (None, None) => Equal,
        (None, Some(_)) => Less,
        (Some(_), None) => Greater,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x
            .as_f64()
            .zip(y.as_f64())
            .and_then(|(x, y)| x.partial_cmp(&y))
            .unwrap_or(Equal),
        (Some(Value::String(x)), Some(Value::String(y))) => x.cmp(y),
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x.cmp(y),
        (Some(x), Some(y)) => kind(x).cmp(&kind(y)),
    }
}

pub(crate) fn set_of<'r>(
    reads: &'r Reads,
    name: &str,
    version: ODataVersion,
) -> Result<&'r Set, ODataErrorResponse> {
    // The URL resolved against the same model, so the set is there.
    reads.edm.sets.get(name).ok_or_else(|| not_found(version))
}

pub(crate) fn resource<'r>(
    reads: &'r Reads,
    name: &str,
    version: ODataVersion,
) -> Result<Resource<'r>, ODataErrorResponse> {
    Resource::new(&reads.state.model, name).ok_or_else(|| not_found(version))
}

/// Whether `Prefer: odata.include-annotations` (Protocol 4.01 8.2.8.4) asks for an annotation
/// of the surface's vocabulary: `*`, or a pattern of `Antares.NgsiLd` or its alias.
fn annotated(headers: &HeaderMap) -> bool {
    headers
        .get_all("Prefer")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|p| {
            let (name, value) = p.trim().split_once('=')?;
            matches!(
                name.trim(),
                "odata.include-annotations" | "include-annotations"
            )
            .then(|| value.trim().trim_matches('"'))
        })
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .any(|p| p == "*" || p.starts_with("Antares.NgsiLd.") || p.starts_with("NgsiLd."))
}

/// `Prefer: odata.maxpagesize`, else [`DEFAULT_PAGE`].
fn max_page(headers: &HeaderMap, max_len: usize) -> u64 {
    headers
        .get("Prefer")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| odata_json::parse_prefer(v, max_len).ok())
        .and_then(|p| p.maxpagesize)
        .unwrap_or(DEFAULT_PAGE)
}

pub(crate) use antares_ops::percent::encode;

/// A 400 of the operation: the data model's refusal (T-5912) and a query on
/// a stripped metadata member (ADR-N-066 2.4) passed on as they are, since
/// they name the attribute and the rule or member and never the value or
/// anything the view hides; any other 400 as [`refused`].
async fn model_refusal(
    reads: &Reads,
    answer: Response,
    version: ODataVersion,
) -> ODataErrorResponse {
    let body = reads.view.read_answer(answer.into_body(), "odata").await;
    let detail = body
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|p| {
            let d = p.get("detail")?.as_str()?;
            (d.contains(linkml_model::check::MARK)
                || d.contains(antares_ops::policy::METADATA_STRIPPED))
            .then(|| d.to_owned())
        });
    match detail {
        Some(d) => {
            ODataErrorResponse::new(StatusCode::BAD_REQUEST, "BadRequest", &d).with_version(version)
        }
        None => refused(StatusCode::BAD_REQUEST, version),
    }
}

/// A refused or failed operation as an OData error with the same status.
/// The text is fixed: no detail of the broker's reaches the caller.
pub(crate) fn refused(status: StatusCode, version: ODataVersion) -> ODataErrorResponse {
    let code = match status {
        StatusCode::NOT_FOUND => "NotFound",
        StatusCode::CONFLICT => "Conflict",
        StatusCode::FORBIDDEN => "Forbidden",
        StatusCode::METHOD_NOT_ALLOWED => "MethodNotAllowed",
        StatusCode::BAD_REQUEST => "BadRequest",
        StatusCode::SERVICE_UNAVAILABLE => "ServiceUnavailable",
        StatusCode::PAYLOAD_TOO_LARGE => "PayloadTooLarge",
        _ => "InternalError",
    };
    ODataErrorResponse::new(status, code, "the request was refused").with_version(version)
}

pub(crate) fn not_found(version: ODataVersion) -> ODataErrorResponse {
    ODataErrorResponse::new(StatusCode::NOT_FOUND, "NotFound", "no such resource")
        .with_version(version)
}

/// `501` naming the `construct` and why (`odata_axum::not_implemented`); a
/// fixed name, never a request value (surfaces-threat-model O19).
pub(crate) fn not_implemented(construct: &str, version: ODataVersion) -> ODataErrorResponse {
    odata_axum::not_implemented(construct).with_version(version)
}

/// A write this surface does not serve: `405` where the method has no
/// meaning (matrix `odata_write_readonly`, `odata_delete_collection`, a
/// `POST` to one Entity), `501` for the forms a later round maps.
pub(crate) fn written(method: &Method, uri: &ODataUri, version: ODataVersion) -> Response {
    let read_only = match uri {
        ODataUri::Resource { path, .. } => match path.as_slice() {
            [Segment::EntitySet(_), Segment::Count] => true,
            [Segment::EntitySet(_), Segment::Query] => method != Method::POST,
            [Segment::EntitySet(_)] => method == Method::DELETE,
            [Segment::EntitySet(_), Segment::Key(_)] => method == Method::POST,
            _ => false,
        },
        // `POST $batch` is served before any other request.
        ODataUri::Batch { .. } => true,
        _ => false,
    };
    if read_only {
        return odata_axum::read_only(version);
    }
    let construct = match uri {
        ODataUri::Resource { path, .. } => match path.as_slice() {
            [Segment::EntitySet(_)] => "a write to a whole entity set (delta update)",
            [.., Segment::Value] => "a write to /$value",
            [.., Segment::Property(_)] => "a write appending to a collection property",
            _ => "a write to this resource path",
        },
        _ => "a write to this resource path",
    };
    not_implemented(construct, version).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edm::Member;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn elements_are_the_members_of_an_array_or_the_one_document() {
        // T-6049: what a whole parse of the answer held, one member at a time
        let read = |text: &str| Elements::<Value>::new(text.as_bytes()).collect::<Vec<_>>();
        let counted = |text: &str| Elements::<serde::de::IgnoredAny>::new(text.as_bytes()).count();
        let array = " [ {\"id\":\"a\"} ,\n{\"id\":[1,2]},3 ] ";
        assert_eq!(
            read(array),
            vec![json!({"id": "a"}), json!({"id": [1, 2]}), json!(3)]
        );
        assert_eq!(counted(array), 3);
        assert_eq!(read("{\"id\":\"a\"}"), vec![json!({"id": "a"})]);
        assert_eq!(read("[]"), Vec::<Value>::new());
        assert_eq!(read("  "), Vec::<Value>::new());
        // a broken member ends them, as a whole parse of a broken answer held none
        assert_eq!(read("[{\"id\":\"a\"},{\"id\":]"), vec![json!({"id": "a"})]);
        assert_eq!(counted("[{\"id\":\"a\"},{\"id\":]"), 1);
    }

    fn set() -> Set {
        let member = |shape| Member {
            iri: "https://e.org/m".into(),
            shape,
            list: false,
            json: false,
            untyped: false,
            written: crate::edm::Written::Always,
        };
        Set {
            output: None,
            type_iri: "https://e.org/T".into(),
            identifier: None,
            references: BTreeMap::new(),
            bound: BTreeMap::new(),
            writable: crate::edm::Writable::default(),
            members: BTreeMap::from([
                ("geo".into(), member(Shape::Geo)),
                (
                    "nav".into(),
                    member(Shape::Nav {
                        target: "T".into(),
                        collection: false,
                    }),
                ),
            ]),
        }
    }

    #[test]
    fn expand_takes_one_level_and_the_objects_of_a_relationship() {
        let edm = Edm {
            csdl: odata_csdl::Model {
                version: odata_csdl::CsdlVersion::V4_01,
                references: vec![],
                schemas: vec![],
            },
            namespace: "n".into(),
            sets: BTreeMap::new(),
            schema_version: "0.m1".into(),
            histories: std::collections::BTreeMap::new(),
        };
        let item = |path: &[&str]| QueryOptions {
            expand: Some(vec![odata_url::ExpandItem {
                path: ExpandPath::Path(path.iter().map(|s| (*s).to_owned()).collect()),
                kind: ExpandKind::Ref,
            }]),
            ..QueryOptions::default()
        };
        let set = set();
        assert!(expanded(&edm, &set, &item(&["c", "nav"]), ODataVersion::V4_0).is_err());
        assert!(
            expanded(&edm, &set, &item(&["nav"]), ODataVersion::V4_0).is_err(),
            "a target set the model lacks"
        );
        assert_eq!(objects(None), Vec::<String>::new());
        let relationship = json!({"object": ["a", 1], "objectList": [{"object": "b"}, "c", 2]});
        assert_eq!(objects(Some(&relationship)), ["a", "b", "c"]);
    }

    #[test]
    fn select_is_a_pick_of_the_frame_and_none_when_an_iri_is_no_member_name() {
        // T-3822: `pick`, never `attrs`, which drops the Entities holding none
        let mut set = set();
        let none = Translated::default();
        let q = query(&set, Some(&["geo", "createdAt"]), &[], &none);
        assert!(
            q.ends_with(
                "&pick=id,type,scope,createdAt,modifiedAt,https%3A%2F%2Fe.org%2Fm&options=sysAttrs"
            ),
            "{q}"
        );
        assert!(!q.contains("attrs="), "{q}");
        set.members
            .entry("geo".into())
            .and_modify(|geo| geo.iri = "https://e.org/~m".into());
        let q = query(&set, Some(&["geo"]), &[], &none);
        assert!(!q.contains("pick="), "{q}");
        assert!(!q.contains("attrs="), "{q}");
    }

    #[test]
    fn select_takes_plain_names_and_leaves_paths_and_annotations_to_later() {
        let set = set();
        let with = |items| QueryOptions {
            select: Some(items),
            ..QueryOptions::default()
        };
        let path = |p: &[&str]| SelectItem::Path(p.iter().map(|s| (*s).to_owned()).collect());
        let star = with(vec![path(&["geo"]), SelectItem::Star]);
        assert_eq!(selected(&set, star.select.as_ref()), Ok(None));
        let plain = with(vec![path(&["geo"]), path(&["scope"])]);
        assert_eq!(
            selected(&set, plain.select.as_ref()),
            Ok(Some(vec!["geo", "scope"]))
        );
        for refused in [
            with(vec![path(&["nav"])]),
            with(vec![path(&["geo", "x"])]),
            with(vec![SelectItem::Annotation("@Core.Description".into())]),
        ] {
            assert_eq!(
                selected(&set, refused.select.as_ref()),
                Err(()),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn each_shape_reads_its_member_of_the_instance() {
        let value = |collection| Shape::Value {
            edm: "Edm.String",
            collection,
        };
        for (shape, instance, want) in [
            (
                Shape::Geo,
                json!({"value": {"type": "Point"}}),
                json!({"type": "Point"}),
            ),
            (Shape::Json, json!({"json": {"a": 1}}), json!({"a": 1})),
            (
                Shape::Language,
                json!({"languageMap": {"@none": "x", "en": "y"}}),
                json!("x"),
            ),
            (
                Shape::Language,
                json!({"languageMap": {"en": "y"}}),
                Value::Null,
            ),
            (Shape::Vocab, json!({"vocab": "Red"}), json!("Red")),
            (
                value(true),
                json!({"value": [{"@value": "a"}, "b"]}),
                json!(["a", "b"]),
            ),
            (value(true), json!({"value": "a"}), json!(["a"])),
            (value(false), json!({"value": ["a", "b"]}), Value::Null),
            (value(false), json!({"value": []}), Value::Null),
            (value(false), json!({"value": {"a": 1}}), json!({"a": 1})),
            (Shape::Url { collection: true }, json!({}), json!([])),
        ] {
            assert_eq!(
                member_value(&shape, Some(&instance)),
                want,
                "{shape:?} {instance}"
            );
        }
        assert_eq!(member_value(&value(false), None), Value::Null);
        assert_eq!(
            served(&Shape::Language, None),
            (Value::Null, None),
            "absent, no note"
        );
        // T-5905: Edm.Binary as unpadded base64url, Edm.TimeOfDay without NGSI-LD's `Z`
        let typed = |edm, collection| Shape::Value { edm, collection };
        for (shape, stored, want) in [
            (
                typed("Edm.Binary", false),
                json!("dGVzdA=="),
                json!("dGVzdA"),
            ),
            (
                typed("Edm.Binary", true),
                json!(["+/8=", ""]),
                json!(["-_8", ""]),
            ),
            (
                typed("Edm.TimeOfDay", false),
                json!("09:30:00Z"),
                json!("09:30:00"),
            ),
            (
                typed("Edm.TimeOfDay", false),
                json!("09:30:00"),
                json!("09:30:00"),
            ),
            (typed("Edm.Binary", false), json!(7), json!(7)),
        ] {
            assert_eq!(
                served(&shape, Some(&json!({"value": stored}))),
                (want, None)
            );
        }
        assert_eq!(
            served(
                &typed("Edm.Binary", false),
                Some(&json!({"value": "no base64!"}))
            ),
            (Value::Null, Some("the stored value is no base64 text"))
        );
    }

    #[test]
    fn ngsild_lang_is_one_language_tag() {
        let with = |v: &str| QueryOptions {
            custom: vec![("ngsild.lang".to_owned(), Some(v.to_owned()))],
            ..QueryOptions::default()
        };
        assert_eq!(lang_of(&QueryOptions::default()), Ok(None));
        assert_eq!(lang_of(&with("en-GB")), Ok(Some("en-GB")));
        for bad in ["", "1a", "s;k", &"a".repeat(36)] {
            assert_eq!(lang_of(&with(bad)), Err(()), "{bad}");
        }
    }

    #[test]
    fn include_annotations_names_the_vocabulary_or_everything() {
        let prefer = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert("Prefer", v.parse().expect("header"));
            annotated(&h)
        };
        assert!(prefer(r#"odata.include-annotations="*""#));
        assert!(prefer("return=minimal, include-annotations=NgsiLd.*"));
        assert!(prefer(
            r#"odata.include-annotations="Antares.NgsiLd.observedAt""#
        ));
        assert!(!prefer(r#"odata.include-annotations="Core.*""#));
        assert!(!prefer("odata.maxpagesize=5"));
        assert!(!annotated(&HeaderMap::new()));
    }

    #[test]
    fn the_default_instance_is_the_one_without_a_dataset_id() {
        let many = json!([{"datasetId": "urn:d:1", "value": 1}, {"value": 2}]);
        assert_eq!(default_instance(Some(&many)), Some(&json!({"value": 2})));
        let only_named = json!([{"datasetId": "urn:d:1", "value": 1}]);
        assert_eq!(
            default_instance(Some(&only_named)),
            None,
            "T-5206: never a named one"
        );
        assert_eq!(default_instance(Some(&json!([]))), None);
        assert_eq!(default_instance(None), None);
    }

    #[test]
    fn an_entity_names_the_visible_sets_of_its_types() {
        let edm = Edm {
            csdl: odata_csdl::Model {
                version: odata_csdl::CsdlVersion::V4_01,
                references: vec![],
                schemas: vec![],
            },
            namespace: "v".into(),
            sets: BTreeMap::from([("T".into(), set())]),
            schema_version: "0.m1".into(),
            histories: std::collections::BTreeMap::new(),
        };
        let both = json!({"type": ["https://e.org/T", "https://e.org/Hidden"]});
        assert_eq!(visible_types(&edm, &both), vec!["T".to_owned()]);
        assert_eq!(
            visible_types(&edm, &json!({"type": "https://e.org/T"})),
            vec!["T".to_owned()]
        );
        assert_eq!(visible_types(&edm, &json!({})), Vec::<String>::new());
    }

    #[test]
    fn a_refusal_keeps_its_status_and_never_its_detail() {
        for (status, code) in [
            (StatusCode::NOT_FOUND, "NotFound"),
            (StatusCode::FORBIDDEN, "Forbidden"),
            (StatusCode::BAD_REQUEST, "BadRequest"),
            (StatusCode::SERVICE_UNAVAILABLE, "ServiceUnavailable"),
            (StatusCode::INTERNAL_SERVER_ERROR, "InternalError"),
        ] {
            let answer = refused(status, ODataVersion::V4_01).into_response();
            assert_eq!(answer.status(), status, "{code}");
        }
    }

    #[test]
    fn a_token_reads_back_only_under_its_digest() {
        assert_eq!(
            read_token(&token(5, 2, Some(3), 7), 7),
            Some((5, 2, Some(3)))
        );
        assert_eq!(read_token(&token(5, 0, None, 7), 7), Some((5, 0, None)));
        for bad in [
            token(5, 0, None, 8),
            "2.5.0.-".to_owned(),
            format!("2.5.0.-.{:016x}.x", 7),
            format!("2.x.0.-.{:016x}", 7),
            format!("2.5.x.-.{:016x}", 7),
            format!("2.5.0.x.{:016x}", 7),
            "2.5.0.-.7".to_owned(),
            // T-3758: a token of the earlier form, offset.top.digest
            format!("5.-.{:016x}", 7),
            format!("3.5.0.-.{:016x}", 7),
        ] {
            assert_eq!(read_token(&bad, 7), None, "{bad}");
        }
    }

    #[test]
    fn encode_leaves_only_unreserved_characters() {
        assert_eq!(
            encode("https://e.org/a#b c"),
            "https%3A%2F%2Fe.org%2Fa%23b%20c"
        );
        assert_eq!(encode("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn a_key_maps_to_its_urn_and_back() {
        // E2a, T-3720: each identifier type both ways, and a URN outside
        // the pattern by `id`.
        let keyed = |edm| Set {
            identifier: Some(crate::edm::Identifier {
                member: "ID".into(),
                edm,
                prefix: "urn:ngsi-ld:P:".into(),
            }),
            ..set()
        };
        let none = QueryOptions::default();
        let plain = set();
        let single = |l| Key::Single(KeyValue::Literal(l));
        let named = |n: &str, l| Key::Compound(vec![(n.into(), KeyValue::Literal(l))]);
        let guid = "01234567-89ab-cdef-0123-456789abcdef";
        let cases = [
            (
                "Edm.Guid",
                single(Literal::Guid(guid.into())),
                format!("urn:ngsi-ld:P:{guid}"),
            ),
            (
                "Edm.Int64",
                single(Literal::Int(-7)),
                "urn:ngsi-ld:P:-7".to_owned(),
            ),
            (
                "Edm.String",
                single(Literal::String("a b".into())),
                "urn:ngsi-ld:P:a b".to_owned(),
            ),
            (
                "Edm.String",
                named("ID", Literal::String("x".into())),
                "urn:ngsi-ld:P:x".to_owned(),
            ),
        ];
        for (edm, key, urn) in cases {
            let set = keyed(edm);
            assert_eq!(
                key_id(&set, &key, &none).as_deref(),
                Some(urn.as_str()),
                "{edm}"
            );
            assert_eq!(canonical_key(&set, &urn), single_or(&key), "{edm}");
        }
        let by_id = |urn: &str| named("id", Literal::String(urn.into()));
        for (edm, urn) in [
            (
                "Edm.Guid",
                "urn:ngsi-ld:P:01234567-89AB-cdef-0123-456789abcdef",
            ),
            ("Edm.Guid", "urn:ngsi-ld:P:x"),
            ("Edm.Int64", "urn:ngsi-ld:P:07"),
            ("Edm.Int64", "urn:ngsi-ld:P:99999999999999999999"),
            ("Edm.String", "urn:ngsi-ld:Q:1"),
        ] {
            let set = keyed(edm);
            assert_eq!(canonical_key(&set, urn), by_id(urn), "{edm} {urn}");
            assert_eq!(key_id(&set, &by_id(urn), &none).as_deref(), Some(urn));
        }
        // A literal of another type names nothing; a string URN stays one
        // where the key is no string.
        let set = keyed("Edm.Guid");
        assert_eq!(key_id(&set, &single(Literal::Int(1)), &none), None);
        assert_eq!(
            key_id(&set, &named("other", Literal::Guid(guid.into())), &none),
            None
        );
        assert_eq!(
            key_id(&set, &single(Literal::String("urn:x".into())), &none).as_deref(),
            Some("urn:x")
        );
        assert_eq!(
            key_id(
                &keyed("Edm.Int64"),
                &single(Literal::Guid(guid.into())),
                &none
            ),
            None
        );
        // Without an identifier the key is the URN (E2).
        assert_eq!(
            canonical_key(&plain, "urn:x"),
            single(Literal::String("urn:x".into()))
        );
        assert_eq!(
            key_id(&plain, &single(Literal::Guid(guid.into())), &none),
            None
        );
        // A body names the key as JSON.
        assert_eq!(
            body_id(&set, &json!(guid.to_ascii_uppercase())),
            Some(format!("urn:ngsi-ld:P:{guid}"))
        );
        assert_eq!(
            body_id(&keyed("Edm.Int64"), &json!(5)),
            Some("urn:ngsi-ld:P:5".into())
        );
        assert_eq!(body_id(&plain, &json!("urn:x")), Some("urn:x".into()));
        assert_eq!(body_id(&plain, &json!(5)), None);
        assert_eq!(body_id(&set, &json!(null)), None);
    }

    #[test]
    fn a_string_key_keeps_its_case_and_a_broken_escape_decodes_to_nothing() {
        let codes = Set {
            identifier: Some(crate::edm::Identifier {
                member: "code".into(),
                edm: "Edm.String",
                prefix: "urn:ngsi-ld:P:".into(),
            }),
            ..set()
        };
        assert_eq!(
            body_id(&codes, &json!("Ab")),
            Some("urn:ngsi-ld:P:Ab".into())
        );
        assert_eq!(percent_decoded("P%28a%29"), Some("P(a)".into()));
        for broken in ["P%zz", "P%2", "%FF"] {
            assert_eq!(percent_decoded(broken), None, "{broken}");
        }
    }

    fn single_or(key: &Key) -> Key {
        // The canonical form of a named key is the single one.
        match key {
            Key::Compound(pairs) => pairs
                .first()
                .map_or_else(|| key.clone(), |(_, v)| Key::Single(v.clone())),
            Key::Single(_) => key.clone(),
        }
    }

    #[test]
    fn the_surface_sort_puts_nothing_first_and_orders_within_a_kind() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        let v = |v: Value| Some(v);
        for (a, b, want) in [
            (None, None, Equal),
            (None, v(json!(1)), Less),
            (v(json!(1)), None, Greater),
            (v(json!(2)), v(json!(10)), Less),
            (v(json!("b")), v(json!("a")), Greater),
            (v(json!(false)), v(json!(true)), Less),
            // across kinds: by kind, bool < number < string < array < object
            (v(json!(true)), v(json!(0)), Less),
            (v(json!("a")), v(json!([1])), Less),
            (v(json!({"a": 1})), v(json!([1])), Greater),
            (v(Value::Null), v(json!(false)), Less),
        ] {
            assert_eq!(compare(a.as_ref(), b.as_ref()), want, "{a:?} {b:?}");
        }
        // a JSON-LD value object sorts by its @value; ties keep their order
        let doc = |id: &str, at: Value, speed: Value| json!({"id": id, "createdAt": at, "t0": {"type": "Property", "value": speed}});
        let mut docs = vec![
            doc(
                "a",
                json!({"@type": "DateTime", "@value": "2026-01-02"}),
                json!(1),
            ),
            doc("b", json!("2026-01-01"), json!(1)),
            doc("c", json!("2026-01-02"), json!(3)),
        ];
        sort(
            &mut docs,
            &[
                SortKey {
                    by: SortBy::System("createdAt".into()),
                    descending: true,
                },
                SortKey {
                    by: SortBy::Value("t0".into()),
                    descending: false,
                },
            ],
        );
        let ids: Vec<&str> = docs.iter().filter_map(|d| d["id"].as_str()).collect();
        assert_eq!(ids, ["a", "c", "b"]);
    }
}
