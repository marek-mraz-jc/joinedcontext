// SPDX-License-Identifier: EUPL-1.2
//! One Query Entities across many views (T-4494,
//! `docs/analysis/view-union.md`): `GET /x/views/_union/ngsi-ld/v1/entities?views=a,b`
//! runs the read through each named view as `/x/views/{view}` runs it, and
//! merges the pages by Entity id under one cursor. `Antares-Entity-Views` names
//! the view of each Entity in body order; the body stays what the views
//! answered. A view the caller may not read contributes nothing.

use antares_ops::{Caller, OperationId, Ops, OpsRequest, Refusal, ViewOps};
use axum::extract::Request;
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::stream::{self, StreamExt};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex, PoisonError};

/// Where the union answers, below the view root. `_` is no view-id
/// character, so no view shadows it.
pub const ROUTE: &str = "/_union/ngsi-ld/v1/entities";
/// The response header naming the view of each Entity, in body order.
pub const VIEWS_HEADER: &str = "antares-entity-views";
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;
/// Entities buffered across every view at once; each view is asked for its
/// share of it per page.
const ROWS_HELD: usize = 2000;
/// Views asked at the same time.
const WIDTH: usize = 16;
/// The largest page one view may answer.
const PAGE_BYTES: usize = 32 * 1024 * 1024;
/// What a read across views answers once its view pages pass the broker's
/// `max_answer_bytes` together (F-ANT1-001).
const OVER_BUDGET: &str =
    "the views' pages of a read across views are over the broker's max_answer_bytes (64 MiB by default) together: name fewer views or narrow the query";
/// What a read across views answers for a view page past [`PAGE_BYTES`].
const PAGE_TOO_LARGE: &str = "a view's page is over the 32 MiB a read across views takes";
/// The parameters the union reads itself; every other one goes to each view.
const OWN: [&str; 5] = ["views", "cursor", "limit", "offset", "count"];
const LD: &str = "application/ld+json";

/// The variable naming the most views one union read may name (200).
pub const MAX_VIEWS_ENV: &str = "ANTARES_UNION_MAX_VIEWS";
static MAX_VIEWS: LazyLock<usize> =
    LazyLock::new(|| views_bound(std::env::var(MAX_VIEWS_ENV).ok()));

/// The bound `ANTARES_UNION_MAX_VIEWS` names; 200 when it names no positive count.
fn views_bound(var: Option<String>) -> usize {
    var.and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(200)
}

/// The union read of `req` as `caller`.
pub async fn serve(ops: &Ops, caller: &Caller, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    if parts.method != Method::GET && parts.method != Method::HEAD {
        return Refusal::NotAllowed {
            allow: vec![Method::GET, Method::HEAD],
        }
        .ngsi_ld();
    }
    let ask = match Ask::of(&parts) {
        Ok(ask) => ask,
        Err(refusal) => return refusal.ngsi_ld(),
    };
    let Some(json) = wants_json(&parts.headers) else {
        return not_acceptable();
    };
    let budget = Budget::new(ops.max_answer_bytes());
    match run(ops, caller, &parts.headers, &ask, &budget).await {
        Ok(page) => page.answer(&ask, json),
        Err(Stop::Refused(refusal)) => refusal.ngsi_ld(),
        Err(Stop::Answer(answer)) => *answer,
    }
}

/// The union's own parameters and what goes on to each view.
struct Ask {
    views: Vec<String>,
    offsets: Vec<u64>,
    limit: usize,
    count: bool,
    /// The caller's other parameters, as sent.
    rest: Vec<String>,
}

impl Ask {
    fn of(parts: &Parts) -> Result<Self, Refusal> {
        if parts.headers.contains_key("ngsild-tenant") {
            return Err(Refusal::Malformed(
                "a union names no tenant: each view reads its own",
            ));
        }
        if parts.headers.contains_key("ngsild-entitymap") {
            return Err(Refusal::Malformed(crate::NO_MAPS));
        }
        // a query that is no form encoding decodes to nothing, so its
        // `views` counts as percent-encoded below
        let decoded = axum::extract::Query::<Vec<(String, String)>>::try_from_uri(&parts.uri)
            .map(|q| q.0)
            .unwrap_or_default();
        for (key, _) in &decoded {
            match key.as_str() {
                "orderBy" | "collation" => {
                    return Err(Refusal::Malformed(
                        "a read across views is never ordered by orderBy (4.23.1): it is in Entity id order",
                    ))
                }
                "entityMap" => return Err(Refusal::Malformed(crate::NO_MAPS)),
                _ => {}
            }
        }
        let mut rest = Vec::new();
        let mut own_sent = 0usize;
        for pair in parts.uri.query().unwrap_or_default().split('&') {
            let key = pair.split_once('=').map_or(pair, |(k, _)| k);
            if OWN.contains(&key) {
                own_sent = own_sent.saturating_add(1);
            } else if !pair.is_empty() {
                rest.push(pair.to_owned());
            }
        }
        let own: Vec<&(String, String)> = decoded
            .iter()
            .filter(|(k, _)| OWN.contains(&k.as_str()))
            .collect();
        // an encoded `views` would reach each view as its own parameter
        if own.len() != own_sent {
            return Err(Refusal::Malformed("a union parameter is percent-encoded"));
        }
        let get = |name: &str| -> Result<Option<&str>, Refusal> {
            let mut all = own.iter().filter(|(k, _)| k == name);
            match (all.next(), all.next()) {
                (one, None) => Ok(one.map(|(_, v)| v.as_str())),
                _ => Err(Refusal::Malformed("a union parameter is given twice")),
            }
        };
        let mut views: Vec<String> = Vec::new();
        for id in get("views")?.unwrap_or_default().split(',').map(str::trim) {
            if !id.is_empty() && !views.iter().any(|v| v == id) {
                views.push(id.to_owned());
            }
        }
        if views.is_empty() {
            return Err(Refusal::Malformed("views names no view"));
        }
        if views.len() > *MAX_VIEWS {
            return Err(Refusal::Limit(
                "views names more views than ANTARES_UNION_MAX_VIEWS",
            ));
        }
        let limit = match get("limit")? {
            None => DEFAULT_LIMIT,
            Some(v) => v
                .parse()
                .map_err(|_| Refusal::Malformed("limit is no count"))?,
        };
        if limit > MAX_LIMIT {
            return Err(Refusal::Limit(
                "limit is above 1000 for a read across views",
            ));
        }
        if get("offset")?.is_some_and(|o| o != "0") {
            return Err(Refusal::Malformed(
                "a read across views pages with the cursor of its next link, never an offset",
            ));
        }
        let offsets = match get("cursor")? {
            None => vec![0; views.len()],
            Some(c) => c
                .split('.')
                .map(str::parse)
                .collect::<Result<Vec<u64>, _>>()
                .ok()
                .filter(|o| o.len() == views.len())
                .ok_or(Refusal::Malformed("cursor does not fit views"))?,
        };
        Ok(Self {
            views,
            offsets,
            limit,
            count: get("count")? == Some("true"),
            rest,
        })
    }

    /// The query of one view's page.
    fn target(&self, offset: u64, limit: usize, count: bool) -> String {
        let mut q = self.rest.clone();
        q.push(format!("limit={limit}"));
        q.push(format!("offset={offset}"));
        if count {
            q.push("count=true".to_owned());
        }
        format!("/ngsi-ld/v1/entities?{}", q.join("&"))
    }
}

/// Whether the caller takes plain JSON (`Some(true)`), JSON-LD
/// (`Some(false)`) or neither.
fn wants_json(headers: &HeaderMap) -> Option<bool> {
    let Some(accept) = headers.get(header::ACCEPT) else {
        return Some(true);
    };
    let accept = accept.to_str().ok()?;
    if accept.contains(LD) {
        Some(false)
    } else if ["application/json", "*/*", "application/*"]
        .iter()
        .any(|m| accept.contains(m))
    {
        Some(true)
    } else {
        None
    }
}

fn not_acceptable() -> Response {
    (
        StatusCode::NOT_ACCEPTABLE,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({"availableRepresentations": [LD]}).to_string(),
    )
        .into_response()
}

/// One view's part of the union.
struct Part {
    view: Option<ViewOps>,
    /// How far the union has read this view.
    read: u64,
    buf: VecDeque<Value>,
    done: bool,
}

/// One page of the union: each Entity with the index of its view.
struct Page {
    docs: Vec<(Value, usize)>,
    ids: Vec<String>,
    count: Option<u64>,
    next: Option<String>,
}

/// What ends a union early: a refusal of the operation layer, or the answer
/// a view gave that every view would give alike.
enum Stop {
    Refused(Refusal),
    Answer(Box<Response>),
}

impl From<Refusal> for Stop {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

/// The bytes of view pages one union request may still read: every view read
/// of the request takes from the one budget, frame by frame, so the pages it
/// holds at once stay under the broker's `max_answer_bytes` (F-ANT1-001).
struct Budget(Mutex<usize>);

impl Budget {
    fn new(bytes: usize) -> Self {
        Self(Mutex::new(bytes))
    }

    /// Takes the `n` bytes of a frame that follows `read` bytes of one
    /// view's page; the bound passed, taking nothing, when the page would
    /// pass [`PAGE_BYTES`] or fewer are left.
    fn take(&self, read: usize, n: usize) -> Result<(), &'static str> {
        if read.saturating_add(n) > PAGE_BYTES {
            return Err(PAGE_TOO_LARGE);
        }
        let mut left = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        *left = left.checked_sub(n).ok_or(OVER_BUDGET)?;
        Ok(())
    }
}

/// One view's page: its Entities and count, `None` when it gives nothing.
type Fetched = Result<Option<(Vec<Value>, u64)>, Stop>;

async fn run(
    ops: &Ops,
    caller: &Caller,
    headers: &HeaderMap,
    ask: &Ask,
    budget: &Budget,
) -> Result<Page, Stop> {
    let mut parts: Vec<Part> = ask
        .views
        .iter()
        .zip(&ask.offsets)
        .map(|(id, read)| {
            let view = ops
                .mapped_view(id)
                .filter(|v| v.allows(OperationId::QueryEntities));
            Part {
                done: view.is_none(),
                view,
                read: *read,
                buf: VecDeque::new(),
            }
        })
        .collect();
    let live = parts.iter().filter(|p| !p.done).count().max(1);
    let chunk = ROWS_HELD
        .checked_div(live)
        .unwrap_or(1)
        .clamp(1, ask.limit.max(1));
    let first = if ask.limit == 0 { 0 } else { chunk };
    let mut total = 0u64;
    if ask.limit > 0 || ask.count {
        let r = Read {
            caller,
            headers,
            ask,
            budget,
        };
        refill(&mut parts, &r, first, ask.count, &mut total).await?;
    }
    let mut docs = Vec::new();
    while docs.len() < ask.limit {
        let r = Read {
            caller,
            headers,
            ask,
            budget,
        };
        refill(&mut parts, &r, chunk, false, &mut total).await?;
        let head = parts
            .iter_mut()
            .enumerate()
            .filter(|(_, p)| !p.buf.is_empty())
            .min_by(|(i, a), (j, b)| {
                (a.buf.front().map(id_of), i).cmp(&(b.buf.front().map(id_of), j))
            });
        let Some((i, part)) = head else {
            break;
        };
        part.read = part.read.saturating_add(1);
        docs.extend(part.buf.pop_front().map(|doc| (doc, i)));
    }
    let more = ask.limit > 0 && parts.iter().any(|p| !p.buf.is_empty() || !p.done);
    let next = more.then(|| {
        parts
            .iter()
            .map(|p| p.read.to_string())
            .collect::<Vec<_>>()
            .join(".")
    });
    Ok(Page {
        ids: ask.views.clone(),
        docs,
        count: ask.count.then_some(total),
        next,
    })
}

fn id_of(doc: &Value) -> &str {
    doc.get("id").and_then(Value::as_str).unwrap_or_default()
}

/// Read the next `limit` Entities of every part whose buffer ran out, at
/// most [`WIDTH`] views at a time. A limit of 0 reads counts alone.
/// What every view read of one union request shares.
struct Read<'a> {
    caller: &'a Caller,
    headers: &'a HeaderMap,
    ask: &'a Ask,
    budget: &'a Budget,
}

async fn refill(
    parts: &mut [Part],
    r: &Read<'_>,
    limit: usize,
    count: bool,
    total: &mut u64,
) -> Result<(), Stop> {
    let empty = |p: &Part| p.buf.is_empty() && !p.done;
    // a part without a view is done from the start
    let jobs: Vec<(ViewOps, u64)> = parts
        .iter()
        .filter(|p| empty(p))
        .filter_map(|p| p.view.clone().map(|v| (v, p.read)))
        .collect();
    let answers: Vec<Fetched> =
        stream::iter(jobs)
            .map(|(view, offset)| async move {
                read(&view, r, r.ask.target(offset, limit, count)).await
            })
            .buffered(WIDTH)
            .collect()
            .await;
    for (part, answer) in parts.iter_mut().filter(|p| empty(p)).zip(answers) {
        match answer? {
            Some((docs, n)) => {
                *total = total.saturating_add(n);
                part.done = limit == 0 || docs.len() < limit;
                part.buf.extend(docs);
            }
            None => part.done = true,
        }
    }
    Ok(())
}

/// One view's page as a direct read through it answers: its Entities and
/// count, `None` when the view gives the caller nothing (unknown, refused,
/// out of reach: no sign which), or the answer that ends the union (a
/// request every view refuses alike, a failure inside the broker).
async fn read(view: &ViewOps, r: &Read<'_>, target: String) -> Fetched {
    let mut req = OpsRequest::new(Method::GET, target);
    for (name, value) in r.headers {
        if name != header::ACCEPT {
            req = req.header(name.clone(), value.clone());
        }
    }
    let req = req.header(header::ACCEPT, HeaderValue::from_static(LD));
    // the union asks each view only for an operation it allows, without a
    // Tenant, so what the layer refuses it refuses for every view alike
    let answer = view.run(OperationId::QueryEntities, r.caller, req).await?;
    match answer.status() {
        StatusCode::OK => {}
        // the view's Policy refuses the caller: no sign which view it was
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
            return Ok(None)
        }
        _ => return Err(Stop::Answer(Box::new(answer))),
    }
    let count = answer
        .headers()
        .get("ngsild-results-count")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = answer.into_body().into_data_stream();
    let mut bytes = Vec::new();
    while let Some(frame) = body.next().await {
        let frame = frame.map_err(|_| Refusal::Limit(PAGE_TOO_LARGE))?;
        r.budget
            .take(bytes.len(), frame.len())
            .map_err(Refusal::Limit)?;
        bytes.extend_from_slice(&frame);
    }
    let docs = serde_json::from_slice(&bytes).map_err(|_| Refusal::Limit(PAGE_TOO_LARGE))?;
    Ok(Some((docs, count)))
}

impl Page {
    fn answer(self, ask: &Ask, json: bool) -> Response {
        let tags: Vec<&str> = self
            .docs
            .iter()
            .filter_map(|(_, i)| self.ids.get(*i).map(String::as_str))
            .collect();
        let mut docs: Vec<Value> = self.docs.into_iter().map(|(d, _)| d).collect();
        let mut response_headers = HeaderMap::new();
        let content = if json {
            // every view compacts under the caller's Link or the core
            // @context alike, so the first Entity names the page's
            let link = docs
                .first()
                .and_then(|d| d.get("@context"))
                .and_then(Value::as_str)
                .map(|url| {
                    format!("<{url}>; rel=\"http://www.w3.org/ns/json-ld#context\"; type=\"{LD}\"")
                });
            for o in docs.iter_mut().filter_map(Value::as_object_mut) {
                o.remove("@context");
            }
            response_headers.extend(
                link.and_then(|l| HeaderValue::from_str(&l).ok())
                    .map(|v| (header::LINK, v)),
            );
            "application/json"
        } else {
            LD
        };
        response_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content));
        // view ids are `[a-z0-9-]`, so the list is a header value
        response_headers.extend(
            HeaderValue::from_str(&tags.join(","))
                .ok()
                .map(|v| (axum::http::HeaderName::from_static(VIEWS_HEADER), v)),
        );
        if let Some(n) = self.count {
            response_headers.insert("ngsild-results-count", HeaderValue::from(n));
        }
        if let Some(cursor) = self.next {
            let mut q: Vec<String> = ask.rest.clone();
            q.push(format!("views={}", ask.views.join(",")));
            q.push(format!("limit={}", ask.limit));
            if ask.count {
                q.push("count=true".to_owned());
            }
            q.push(format!("cursor={cursor}"));
            let link = format!(
                "<{}{ROUTE}?{}>; rel=\"next\"; type=\"{LD}\"",
                crate::PREFIX,
                q.join("&")
            );
            response_headers.extend(HeaderValue::from_str(&link).ok().map(|v| (header::LINK, v)));
        }
        let mut response = (StatusCode::OK, Value::Array(docs).to_string()).into_response();
        response.headers_mut().extend(response_headers);
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_is_taken_until_it_is_spent() {
        let budget = Budget::new(10);
        assert_eq!(budget.take(0, 6), Ok(()));
        assert_eq!(budget.take(6, 5), Err(OVER_BUDGET), "5 of 4 left");
        assert_eq!(budget.take(6, 4), Ok(()));
        assert_eq!(budget.take(10, 1), Err(OVER_BUDGET), "nothing left");
        assert_eq!(budget.take(10, 0), Ok(()));
        let pages = Budget::new(PAGE_BYTES.saturating_mul(2));
        assert_eq!(pages.take(PAGE_BYTES, 1), Err(PAGE_TOO_LARGE));
        assert_eq!(pages.take(0, PAGE_BYTES), Ok(()));
    }

    fn ask(query: &str) -> Option<Ask> {
        let req = axum::http::Request::builder()
            .uri(format!("/x/views{ROUTE}?{query}"))
            .body(())
            .unwrap_or_default();
        Ask::of(&req.into_parts().0).ok()
    }

    #[test]
    fn the_union_reads_its_own_parameters_and_passes_the_rest() {
        let a = ask("type=Car&views=b,a,b,%20&&q=speed%3E1&count=true&limit=5");
        assert_eq!(
            a.as_ref()
                .map(|a| (&a.views[..], &a.offsets[..], a.limit, a.count, &a.rest[..])),
            Some((
                &["b".to_owned(), "a".to_owned()][..],
                &[0, 0][..],
                5,
                true,
                &["type=Car".to_owned(), "q=speed%3E1".to_owned()][..]
            ))
        );
        assert_eq!(
            a.map(|a| a.target(7, 3, false)).as_deref(),
            Some("/ngsi-ld/v1/entities?type=Car&q=speed%3E1&limit=3&offset=7")
        );
        let c = ask("views=a,b&cursor=4.0&offset=0");
        assert_eq!(
            c.map(|c| (c.offsets, c.limit, c.count)),
            Some((vec![4, 0], DEFAULT_LIMIT, false))
        );
    }

    #[test]
    fn what_a_union_cannot_honour_is_refused() {
        for query in [
            "type=Car",
            "views=",
            "views=a&orderBy=id",
            "views=a&collation=de",
            "views=a&offset=2",
            "views=a&limit=1001",
            "views=a&limit=x",
            "views=a,b&cursor=1",
            "views=a&cursor=x",
            "views=a&views=b",
            "vi%65ws=a",
            "views=a&entityMap=true",
        ] {
            assert!(ask(query).is_none(), "{query}");
        }
        let many: Vec<String> = (0..=200).map(|i| format!("v{i}")).collect();
        assert!(ask(&format!("views={}", many.join(","))).is_none());
        assert!(ask("views=a&offset=0&limit=0").is_some());
    }

    #[test]
    fn the_views_bound_is_a_positive_count_or_200() {
        assert_eq!(views_bound(Some("5".into())), 5);
        for var in [None, Some("0".into()), Some("x".into())] {
            assert_eq!(views_bound(var), 200);
        }
    }

    #[test]
    fn json_json_ld_or_nothing() {
        let accept = |v: &'static str| {
            let mut h = HeaderMap::new();
            h.insert(header::ACCEPT, HeaderValue::from_static(v));
            wants_json(&h)
        };
        assert_eq!(wants_json(&HeaderMap::new()), Some(true));
        assert_eq!(accept("application/ld+json"), Some(false));
        assert_eq!(accept("*/*"), Some(true));
        assert_eq!(accept("application/geo+json"), None);
        let mut h = HeaderMap::new();
        h.insert(
            header::ACCEPT,
            HeaderValue::from_bytes(b"\xff").unwrap_or(HeaderValue::from_static("x/y")),
        );
        assert_eq!(wants_json(&h), None);
    }
}
