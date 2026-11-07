// SPDX-License-Identifier: EUPL-1.2
//! The Events of one view as iCalendar feeds (RFC 5545, RFC 7986):
//! `/x/views/{view}/ical/v1/{output}.ics?from=…&to=…&q=…` (T-4487, T-4901, T-5208,
//! `docs/analysis/ical-mapping.md`).
//!
//! A view serves a calendar only when its `representations.ical` lists named outputs (owner
//! 2026-10-22T09:25Z and 09:30Z): none is the 404 an unknown view answers. `events.ics` is the
//! output named `events`, else the first (deprecated alias of the single feed of T-4487). An
//! output renders its sources' field mappings over one NGSI-LD Query Entities of their types
//! through the view, narrowed by the output's `filter` (its `q` and `scopeQ` AND-ed with a
//! caller's, its geo-query fixed): `q`, `scopeQ`, `georel`, `geometry`, `coordinates`,
//! `geoproperty`, `id`, `idPattern`, `lang` and `attrs` pass through as Query Entities reads
//! them, so a type or an Attribute the view hides is absent and the Policy decides as for
//! NGSI-LD. Any other parameter is `400`. A view that shows no Entity of the types, or does not
//! allow Query Entities, has no feed. The calendar carries `NAME` per title language,
//! `REFRESH-INTERVAL`, the output zone's `VTIMEZONE`, and a strong `ETag` (`If-None-Match`:
//! 304). An Entity the mapping cannot render is left out and counted in
//! `antares_surface_records_skipped_total{surface="ical",reason}` (EP-126), never served as an
//! invalid `VEVENT`.
//!
//! `POST /x/views/{view}/ical/v1/preview` renders a candidate mapping over the first Entities
//! the request's subject reads through the view, for the Portal's Endpoint form: only with the
//! broker's admin bearer token (the Portal's), nothing stored.
//!
//! ```
//! use antares_ops::Surface;
//! let ical = antares_ical::Ical;
//! assert_eq!(ical.name(), "ical");
//! assert_eq!(ical.prefix(), "/x/views");
//! ```

#![forbid(unsafe_code)]

pub mod mapping;
pub mod zone;

use antares_ops::percent::{decoded, encode};
use antares_ops::{
    Caller, OperationId, Ops, Page, Problem, Pushdown, Scanned, Surface, ViewOps, RESIDUAL_SCAN,
};
use axum::extract::{Query, Request};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::any;
use axum::Router;
use chrono::Utc;
use mapping::{fold, local, CompiledOutput, IcalMapping, Outcome, Plan, When, Window};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Where the broker mounts the surface; the view and `ical/v1/events.ics` follow.
pub const PREFIX: &str = "/x/views";

/// The metric an Event left out of a feed increments (EP-126).
pub const SKIPPED: &str = "antares_surface_records_skipped_total";

/// The header a feed reports how many Events it left out in.
pub const SKIPPED_HEADER: &str = "antares-skipped-records";

/// The Query Entities parameters a feed passes through as they are (TS 104 175 6.4.3.2).
pub const PASSED: [&str; 9] = [
    "q",
    "scopeQ",
    "georel",
    "geometry",
    "coordinates",
    "geoproperty",
    "id",
    "idPattern",
    "lang",
];

/// The largest preview request body, in bytes.
pub const MAX_PREVIEW_BYTES: usize = 16 * 1024;

/// The most Entities one preview renders.
pub const MAX_PREVIEW_LIMIT: usize = 50;

/// How many `VEVENT`s a preview answers.
const PREVIEW_EVENTS: usize = 3;

/// The Entities one upstream page reads.
const PAGE: usize = 100;

/// The iCalendar surface.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ical;

impl Surface for Ical {
    fn name(&self) -> &'static str {
        "ical"
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn version_info(&self) -> Value {
        json!({ "endpoint": format!("{PREFIX}/{{view}}/ical/v1/events.ics"), "conformsTo": ["https://www.rfc-editor.org/rfc/rfc5545"] })
    }

    fn router(&self, ops: Ops) -> Router {
        let preview_ops = ops.clone();
        Router::new()
            .route(
                "/{view}/ical/v1/{file}",
                any(move |caller: Caller, req: Request| {
                    let ops = ops.clone();
                    async move { serve(&ops, &caller, req).await }
                }),
            )
            .route(
                "/{view}/ical/v1/preview",
                any(move |caller: Caller, req: Request| {
                    let ops = preview_ops.clone();
                    async move { preview(&ops, &caller, req).await }
                }),
            )
    }
}

fn not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "no such calendar")
}

async fn serve(ops: &Ops, caller: &Caller, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let refused =
        || Problem::new(StatusCode::METHOD_NOT_ALLOWED, "this surface is read only").reply();
    antares_ops::read_only(&parts.method, refused, async {
        let Some(name) = file_segment(&parts.uri).strip_suffix(".ics") else {
            return not_found().reply();
        };
        match feed(ops, caller, view_segment(&parts.uri), name, &parts.uri).await {
            Ok((body, skipped)) => tagged(&parts.headers, body, skipped),
            Err(problem) => problem.reply(),
        }
    })
    .await
}

/// The feed with a strong `ETag` of its bytes, or 304 when `If-None-Match` names it.
fn tagged(headers: &HeaderMap, body: String, skipped: usize) -> Response {
    let digest = Sha256::digest(body.as_bytes());
    let mut etag = String::from("\"");
    for b in digest {
        let _ = write!(etag, "{b:02x}");
    }
    etag.push('"');
    let matched = headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().trim_start_matches("W/"))
        .any(|t| t == "*" || t == etag);
    let mut reply = if matched {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        (
            StatusCode::OK,
            [
                (CONTENT_TYPE, "text/calendar; charset=utf-8".to_owned()),
                (HeaderName::from_static(SKIPPED_HEADER), skipped.to_string()),
            ],
            body,
        )
            .into_response()
    };
    let h = reply.headers_mut();
    // a quoted hex digest is always a header value; the fallback is never sent
    h.insert(
        ETAG,
        HeaderValue::from_str(&etag).unwrap_or(HeaderValue::from_static("\"\"")),
    );
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    reply
}

/// `/{view}/ical/v1/…`: a view id is `[a-z0-9-]`, so its raw segment is its id.
fn view_segment(uri: &axum::http::Uri) -> &str {
    uri.path().split('/').nth(1).unwrap_or_default()
}

/// `/{view}/ical/v1/{file}`: the last segment.
fn file_segment(uri: &axum::http::Uri) -> &str {
    uri.path().rsplit('/').next().unwrap_or_default()
}

/// A feed's query string read: the window, what passes through to Query Entities, the `type`
/// a caller named and the `attrs` it selects.
#[derive(Debug, Default)]
struct Asked {
    from: Option<When>,
    to: Option<When>,
    passed: BTreeMap<String, String>,
    types: Vec<String>,
    attrs: Option<Vec<String>>,
}

fn asked(uri: &axum::http::Uri) -> Result<Asked, Problem> {
    let bad = |title: String| Problem::new(StatusCode::BAD_REQUEST, title);
    // a list of pairs reads any query string (lossy UTF-8): this never fails
    let pairs = Query::<Vec<(String, String)>>::try_from_uri(uri)
        .map(|Query(p)| p)
        .unwrap_or_default();
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (key, value) in &pairs {
        if seen.insert(key, value).is_some() {
            return Err(bad(format!("{key} is given more than once")));
        }
    }
    let date = |name: &str| -> Result<Option<When>, Problem> {
        seen.get(name)
            .map(|v| {
                When::parse(v)
                    .ok_or_else(|| bad("from and to take an RFC 3339 date or date-time".to_owned()))
            })
            .transpose()
    };
    let mut out = Asked {
        from: date("from")?,
        to: date("to")?,
        ..Asked::default()
    };
    if let (Some(from), Some(to)) = (out.from, out.to) {
        if from.utc_key(chrono_tz::UTC) >= to.utc_past(chrono_tz::UTC) {
            return Err(bad("from is after to".to_owned()));
        }
    }
    let list = |v: &str| v.split(',').map(str::to_owned).collect::<Vec<_>>();
    for (key, value) in seen {
        match key {
            "from" | "to" => {}
            "type" => out.types = list(value),
            "attrs" => out.attrs = Some(list(value)),
            k if PASSED.contains(&k) => {
                out.passed.insert(k.to_owned(), value.to_owned());
            }
            other => {
                return Err(bad(format!(
                    "{other:?} is not a parameter of this feed: from, to, type, attrs, {}",
                    PASSED.join(", ")
                )))
            }
        }
    }
    Ok(out)
}

/// The view's output `name` compiled (`events`: that one, else the first); a view without a
/// list has no calendar. An output that does not compile serves nobody, and the reason is the
/// preview's to tell (the Portal checks it at propose).
fn output(view: &ViewOps, name: &str) -> Result<CompiledOutput, Problem> {
    let written = view.representation("ical").ok_or_else(not_found)?;
    let broken = || {
        Problem::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the view's calendar outputs do not hold; its Endpoint must be published again",
        )
    };
    let outputs = written.as_array().ok_or_else(broken)?;
    let chosen = outputs
        .iter()
        .find(|o| o.get("name").and_then(Value::as_str) == Some(name))
        .or_else(|| (name == "events").then(|| outputs.first()).flatten())
        .ok_or_else(not_found)?;
    let (written, _) = mapping::output_of(chosen).map_err(|_| broken())?;
    CompiledOutput::compile(&written).map_err(|_| broken())
}

/// What every Query Entities names: the view's types whose local name is a plan's, or for a
/// view of every type the stored types of those names ([`ViewOps::type_selector`]). A
/// caller's `type` may only name those types.
async fn selector(view: &ViewOps, wanted: &[&str], types: &[String]) -> Result<String, Problem> {
    if types.iter().any(|t| !wanted.contains(&local(t))) {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            format!("this calendar maps the types {} alone", wanted.join(", ")),
        ));
    }
    view.type_selector(wanted).await.ok_or_else(not_found)
}

/// The caller's query parameters under the output's filter: `q` and `scopeQ` AND-ed, a fixed
/// geo-query never replaced, so the output only narrows what the view shows.
fn narrowed(filter: &mapping::Filter, asked: &Asked) -> Result<String, Problem> {
    let mut params = asked.passed.clone();
    filter
        .narrow(&mut params)
        .map_err(|why| Problem::new(StatusCode::BAD_REQUEST, why))?;
    let mut out = String::new();
    for (k, v) in params {
        let _ = write!(out, "&{k}={}", encode(&v));
    }
    Ok(out)
}

/// The view of `id` when it allows Query Entities; a view that does not has no feed, as an
/// unknown one.
fn view_of(ops: &Ops, id: &str) -> Result<ViewOps, Problem> {
    let id = decoded(id).ok_or_else(not_found)?;
    ops.view(&id)
        .filter(|v| v.allows(OperationId::QueryEntities))
        .ok_or_else(not_found)
}

/// The pushdown of a feed or a preview over `plans`.
async fn pushdown(
    view: &ViewOps,
    plans: &[&Plan],
    filter: &mapping::Filter,
    asked: &Asked,
) -> Result<Pushdown, Problem> {
    let wanted: Vec<&str> = plans.iter().map(|p| p.entity_type()).collect();
    Ok(Pushdown {
        query: format!(
            "{}{}",
            selector(view, &wanted, &asked.types).await?,
            narrowed(filter, asked)?
        ),
        attrs: asked.attrs.clone(),
        sys_attrs: true,
        ..Pushdown::default()
    })
}

/// One upstream page: its Entities of the plans' types and the next offset.
async fn page(
    view: &ViewOps,
    caller: &Caller,
    pushdown: &Pushdown,
    plans: &[&Plan],
    page: Page,
) -> Result<(Vec<Value>, Option<usize>), Problem> {
    let (entities, next) = match view.scan(caller, pushdown, None, page).await {
        Ok(Scanned::Page { entities, next, .. }) => (entities, next),
        // A Tenant that holds nothing yet answers 404 (5.5.10): an empty calendar.
        Ok(Scanned::Answer(a)) if a.status() == StatusCode::NOT_FOUND => (Vec::new(), None),
        Ok(Scanned::Answer(a)) => return Err(Problem::Answer(Box::new(a))),
        Err(refusal) => return Err(Problem::Refused(refusal)),
    };
    let typed = entities
        .into_iter()
        .filter(|e| {
            let ty = local(e.get("type").and_then(Value::as_str).unwrap_or_default());
            plans.iter().any(|p| p.entity_type() == ty)
        })
        .collect();
    Ok((typed, next))
}

/// The calendar's own properties: RFC 7986 `NAME` and `DESCRIPTION` per language,
/// `REFRESH-INTERVAL`, and the zone (`X-WR-*` for the clients that read only those).
fn header(out: &CompiledOutput, view: &str, mut push: impl FnMut(&str)) {
    for line in [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Antares//antares-ical//EN",
        "CALSCALE:GREGORIAN",
    ] {
        push(line);
    }
    let o = &out.output;
    let preferred = |texts: &BTreeMap<String, String>| {
        o.lang
            .as_ref()
            .and_then(|l| texts.get(l))
            .or_else(|| texts.values().next())
            .cloned()
    };
    for (tag, title) in &o.title {
        push(&format!("NAME;LANGUAGE={tag}:{}", mapping::escape(title)));
    }
    for (tag, text) in &o.description {
        push(&format!(
            "DESCRIPTION;LANGUAGE={tag}:{}",
            mapping::escape(text)
        ));
    }
    let name = preferred(&o.title).unwrap_or_else(|| view.to_owned());
    push(&format!("X-WR-CALNAME:{}", mapping::escape(&name)));
    push("REFRESH-INTERVAL;VALUE=DURATION:PT6H");
    push("X-PUBLISHED-TTL:PT6H");
    push(&format!("X-WR-TIMEZONE:{}", out.zone.name()));
}

/// The feed of output `name` of view `id` and how many Events it left out.
async fn feed(
    ops: &Ops,
    caller: &Caller,
    id: &str,
    name: &str,
    uri: &axum::http::Uri,
) -> Result<(String, usize), Problem> {
    let asked = asked(uri)?;
    let view = view_of(ops, id)?;
    let output = output(&view, name)?;
    let plans: Vec<&Plan> = output.plans.iter().collect();
    let pushdown = pushdown(&view, &plans, &output.output.filter, &asked).await?;
    let window = Window::read(asked.from, asked.to, output.zone);
    let now = Utc::now();
    let mut out = String::new();
    header(&output, &decoded(id).unwrap_or_default(), |l| {
        fold(l, &mut out);
    });
    for line in zone::vtimezone(output.zone, zone::year(now)) {
        fold(&line, &mut out);
    }
    let mut events: Vec<(Option<chrono::NaiveDateTime>, String)> = Vec::new();
    let (mut start, mut skipped) = (0usize, 0usize);
    // ponytail: the window is applied after the read, so a feed reads every Event the query
    // selects up to RESIDUAL_SCAN; a caller narrows with q (startDate>=…) past that.
    loop {
        let at = Page {
            start,
            size: PAGE,
            whole: false,
        };
        let (entities, next) = page(&view, caller, &pushdown, &plans, at).await?;
        // `page` keeps the Entities of the plans' types alone
        let planned = entities
            .iter()
            .filter_map(|e| output.plan(e).map(|p| (p, e)));
        for (plan, entity) in planned {
            match plan.vevent(entity, &window, now) {
                Outcome::Event(text) => events.push((plan.start(entity), text)),
                Outcome::Outside => {}
                Outcome::Skipped(reason) => {
                    skipped = skipped.saturating_add(1);
                    metrics::counter!(SKIPPED, "surface" => "ical", "reason" => reason)
                        .increment(1);
                }
            }
        }
        match next {
            Some(n) if n >= RESIDUAL_SCAN => {
                return Err(Problem::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "the query selects as many events as one feed reads or more: narrow it with q",
                ))
            }
            Some(n) => start = n,
            None => break,
        }
    }
    let o = &output.output;
    if o.sort.is_some() || o.limit.is_some() {
        // stable: Events that start together keep the view's order
        events.sort_by(|a, b| {
            if output.descending {
                b.0.cmp(&a.0)
            } else {
                a.0.cmp(&b.0)
            }
        });
    }
    let keep = o
        .limit
        .map_or(usize::MAX, |n| usize::try_from(n).unwrap_or(usize::MAX));
    for (_, text) in events.into_iter().take(keep) {
        out.push_str(&text);
    }
    fold("END:VCALENDAR", &mut out);
    Ok((out, skipped))
}

/// The body of `POST …/ical/v1/preview`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewBody {
    /// The candidate `representations.ical`.
    mapping: Value,
    /// How many Entities to render, 1 to [`MAX_PREVIEW_LIMIT`] (the default).
    #[serde(default)]
    limit: Option<usize>,
}

async fn preview(ops: &Ops, caller: &Caller, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    // before anything else, so a caller without the token learns nothing about the view
    if !ops.admits_admin(&parts.headers) {
        return Problem::new(StatusCode::FORBIDDEN, "the preview serves the Portal alone").reply();
    }
    if parts.method != Method::POST {
        return Problem::new(StatusCode::METHOD_NOT_ALLOWED, "a preview is a POST").reply();
    }
    let Ok(bytes) = axum::body::to_bytes(body, MAX_PREVIEW_BYTES).await else {
        return Problem::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("a preview body is at most {MAX_PREVIEW_BYTES} bytes"),
        )
        .reply();
    };
    match previewed(ops, caller, view_segment(&parts.uri), &parts.uri, &bytes).await {
        Ok(answer) => (StatusCode::OK, axum::Json(answer)).into_response(),
        Err(problem) => problem.reply(),
    }
}

/// The counts and the first `VEVENT`s of a candidate mapping over the first Entities read.
async fn previewed(
    ops: &Ops,
    caller: &Caller,
    id: &str,
    uri: &axum::http::Uri,
    bytes: &[u8],
) -> Result<Value, Problem> {
    let bad = |title: String| Problem::new(StatusCode::BAD_REQUEST, title);
    let body: PreviewBody =
        serde_json::from_slice(bytes).map_err(|e| bad(format!("not a preview body: {e}")))?;
    let limit = body.limit.unwrap_or(MAX_PREVIEW_LIMIT);
    if !(1..=MAX_PREVIEW_LIMIT).contains(&limit) {
        return Err(bad(format!("limit is 1 to {MAX_PREVIEW_LIMIT}")));
    }
    let candidate = serde_json::from_value::<IcalMapping>(body.mapping)
        .map_err(|e| e.to_string())
        .and_then(|m| Plan::compile(&m))
        .map_err(|why| bad(format!("the mapping does not hold: {why}")))?;
    let asked = asked(uri)?;
    let view = view_of(ops, id)?;
    let plan = &candidate;
    let pushdown = pushdown(&view, &[plan], &mapping::Filter::default(), &asked).await?;
    let window = Window::read(asked.from, asked.to, plan.zone());
    let first = Page {
        start: 0,
        size: limit,
        whole: false,
    };
    let (entities, _) = page(&view, caller, &pushdown, &[plan], first).await?;
    let now = Utc::now();
    let (mut valid, mut outside) = (0usize, 0usize);
    let mut skipped: BTreeMap<&str, usize> = BTreeMap::new();
    let mut events = Vec::new();
    for entity in &entities {
        match plan.vevent(entity, &window, now) {
            Outcome::Event(text) => {
                valid = valid.saturating_add(1);
                if events.len() < PREVIEW_EVENTS {
                    events.push(text);
                }
            }
            Outcome::Outside => outside = outside.saturating_add(1),
            Outcome::Skipped(reason) => {
                let n = skipped.entry(reason).or_default();
                *n = n.saturating_add(1);
            }
        }
    }
    Ok(json!({
        "read": entities.len(),
        "valid": valid,
        "outside": outside,
        "skipped": skipped,
        "events": events,
    }))
}
