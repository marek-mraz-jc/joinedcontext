//! `paginate` (ekiben extension, T-7056 G-07): one option on the `http_client` input and the
//! `http` processor that walks a paged HTTP source, so no mapping hand-writes a loop around `http`.
//!
//! ```yaml
//! paginate:
//!   mode: page | offset | cursor | link | next_url
//!   # page:     param (page), start (1), size_param, size
//!   # offset:   offset_param (offset), limit_param (limit), limit, start (0)
//!   # cursor:   cursor (a body path) or cursor_query (`${! … }`), and cursor_param or cursor_header
//!   # link:     RFC 8288 `Link: <…>; rel="next"`
//!   # next_url: next (a body path: `@odata.nextLink`, `links[rel=next].href`, `_links.next`)
//!   records: features        # the page's array (required by page, offset, total, max_records)
//!   total: numberMatched     # the announced count, read from the first page
//!   more: exceededTransferLimit   # a boolean saying another page exists
//!   max_pages: 1000          # 1..=10000
//!   max_records: 50000
//!   delay: 200ms
//!   order_by: {param: orderByFields, value: OBJECTID}
//! ```
//!
//! Each page is one message (`http_page` metadata, 1-based). The walk ends when its stop rule
//! holds: an empty page, the announced total read, `more` false, a short page (page and offset
//! without `more`), or no next cursor or URL. Reaching `max_pages` or `max_records` first, or a
//! total that does not match what was read, fails the run as incomplete. Offset mode moves by the
//! records received, so a server that caps the page below `limit` drops nothing (T-5427).
//!
//! A next URL from the source keeps the configured scheme, host and port and carries no
//! credentials, or the paging step is refused; every page passes the outbound rule as any request.

use std::sync::Arc;
use std::time::Duration;

use ekiben_bloblang::{Environment, FieldExpression, Value};
use ekiben_core::config::parse_duration_str;
use ekiben_core::{
    redact_url_str, Batch, CoreError, OrgContext, Part, PartsView, ProcContext, ResourceManager,
};
use reqwest::header::HeaderMap;
use serde_json::Value as JsonValue;
use url::Url;

use crate::client::{extract_status_code_from_err, HttpClient};

/// The most pages one walk may read.
pub const MAX_PAGES_LIMIT: usize = 10_000;
/// `max_pages` when the config names none.
pub const DEFAULT_MAX_PAGES: usize = 1_000;

/// What one page's request overrides: its whole URL and the headers it adds.
pub struct PageRequest {
    pub url: Url,
    pub headers: Vec<(String, String)>,
}

enum CursorFrom {
    Path(String),
    Query(FieldExpression),
}

enum CursorTarget {
    Param(String),
    Header(String),
}

enum Mode {
    Page {
        param: String,
        start: u64,
        size_param: Option<String>,
        size: Option<u64>,
    },
    Offset {
        offset_param: String,
        limit_param: String,
        limit: u64,
        start: u64,
    },
    Cursor {
        from: CursorFrom,
        to: CursorTarget,
    },
    Link,
    NextUrl {
        next: String,
    },
}

impl Mode {
    /// The page size asked for, which a short page falls under.
    fn page_size(&self) -> Option<u64> {
        match self {
            Mode::Page { size, .. } => *size,
            Mode::Offset { limit, .. } => Some(*limit),
            _ => None,
        }
    }
}

/// A parsed `paginate` option.
pub struct PaginateConf {
    mode: Mode,
    records: Option<String>,
    total: Option<String>,
    more: Option<String>,
    max_pages: usize,
    max_records: Option<u64>,
    delay: Duration,
    order_by: Option<(String, String)>,
}

const COMMON: &[&str] = &[
    "mode",
    "records",
    "total",
    "more",
    "max_pages",
    "max_records",
    "delay",
    "order_by",
];

fn bad(msg: impl std::fmt::Display) -> CoreError {
    CoreError::config(format!("paginate: {msg}"))
}

fn text<'a>(
    map: &'a serde_json::Map<String, JsonValue>,
    key: &str,
) -> Result<Option<&'a str>, CoreError> {
    match map.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(s)) if !s.trim().is_empty() => Ok(Some(s.as_str())),
        Some(_) => Err(bad(format!("{key}: expected a non-empty string"))),
    }
}

fn num(map: &serde_json::Map<String, JsonValue>, key: &str) -> Result<Option<u64>, CoreError> {
    match map.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .map(Some)
            .ok_or_else(|| bad(format!("{key}: expected a whole number"))),
    }
}

impl PaginateConf {
    pub fn from_json(val: &JsonValue, env: &Environment) -> Result<Arc<Self>, CoreError> {
        let map = val.as_object().ok_or_else(|| bad("expected an object"))?;
        let mode_name = text(map, "mode")?
            .ok_or_else(|| bad("mode is required: page, offset, cursor, link or next_url"))?;
        let own: &[&str] = match mode_name {
            "page" => &["param", "start", "size_param", "size"],
            "offset" => &["offset_param", "limit_param", "limit", "start"],
            "cursor" => &["cursor", "cursor_query", "cursor_param", "cursor_header"],
            "link" => &[],
            "next_url" => &["next"],
            other => {
                return Err(bad(format!(
                    "mode '{other}' is not one of page, offset, cursor, link, next_url"
                )))
            }
        };
        if let Some(key) = map
            .keys()
            .find(|k| !COMMON.contains(&k.as_str()) && !own.contains(&k.as_str()))
        {
            return Err(bad(format!(
                "field {key} is not recognised for mode {mode_name}"
            )));
        }
        let records = text(map, "records")?.map(String::from);
        let mode = match mode_name {
            "page" => {
                let size_param = text(map, "size_param")?.map(String::from);
                let size = num(map, "size")?;
                if size_param.is_some() && size.is_none() {
                    return Err(bad("size_param needs size, the page size asked for"));
                }
                Mode::Page {
                    param: text(map, "param")?.unwrap_or("page").to_string(),
                    start: num(map, "start")?.unwrap_or(1),
                    size_param,
                    size,
                }
            }
            "offset" => Mode::Offset {
                offset_param: text(map, "offset_param")?.unwrap_or("offset").to_string(),
                limit_param: text(map, "limit_param")?.unwrap_or("limit").to_string(),
                limit: num(map, "limit")?
                    .filter(|n| *n > 0)
                    .ok_or_else(|| bad("offset mode needs limit, the page size asked for"))?,
                start: num(map, "start")?.unwrap_or(0),
            },
            "cursor" => {
                let from = match (text(map, "cursor")?, text(map, "cursor_query")?) {
                    (Some(path), None) => CursorFrom::Path(path.to_string()),
                    (None, Some(query)) => CursorFrom::Query(
                        env.compile_field(query)
                            .map_err(|e| bad(format!("cursor_query: {e}")))?,
                    ),
                    _ => return Err(bad("set one of cursor (a body path) or cursor_query")),
                };
                let to = match (text(map, "cursor_param")?, text(map, "cursor_header")?) {
                    (Some(param), None) => CursorTarget::Param(param.to_string()),
                    (None, Some(header)) => {
                        reqwest::header::HeaderName::from_bytes(header.as_bytes())
                            .map_err(|_| bad("cursor_header is not a valid header name"))?;
                        CursorTarget::Header(header.to_string())
                    }
                    _ => return Err(bad("set one of cursor_param or cursor_header")),
                };
                Mode::Cursor { from, to }
            }
            "link" => Mode::Link,
            _ => Mode::NextUrl {
                next: text(map, "next")?
                    .ok_or_else(|| bad("next_url mode needs next, the body path to the next URL"))?
                    .to_string(),
            },
        };
        if matches!(mode, Mode::Page { .. } | Mode::Offset { .. }) && records.is_none() {
            return Err(bad(format!(
                "mode {mode_name} needs records, the path to the page's array"
            )));
        }
        let total = text(map, "total")?.map(String::from);
        let max_records = num(map, "max_records")?;
        if max_records == Some(0) {
            return Err(bad("max_records must be at least 1"));
        }
        if (total.is_some() || max_records.is_some()) && records.is_none() {
            return Err(bad("total and max_records count records: set records too"));
        }
        let max_pages = num(map, "max_pages")?
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
            .unwrap_or(DEFAULT_MAX_PAGES);
        if max_pages == 0 || max_pages > MAX_PAGES_LIMIT {
            return Err(bad(format!(
                "max_pages must be between 1 and {MAX_PAGES_LIMIT}"
            )));
        }
        let delay = text(map, "delay")?
            .map(parse_duration_str)
            .transpose()?
            .unwrap_or_default();
        let order_by = match map.get("order_by") {
            None | Some(JsonValue::Null) => None,
            Some(JsonValue::Object(o)) => match (text(o, "param")?, text(o, "value")?) {
                (Some(p), Some(v)) => Some((p.to_string(), v.to_string())),
                _ => return Err(bad("order_by needs param and value")),
            },
            Some(_) => return Err(bad("order_by: expected {param, value}")),
        };
        Ok(Arc::new(Self {
            mode,
            records,
            total,
            more: text(map, "more")?.map(String::from),
            max_pages,
            max_records,
            delay,
            order_by,
        }))
    }
}

/// The state of one walk over a paged source.
pub struct Walk {
    conf: Arc<PaginateConf>,
    base: Option<Url>,
    next_url: Option<Url>,
    cursor: Option<String>,
    page: u64,
    seen: u64,
    total: Option<u64>,
    done: bool,
}

impl Walk {
    pub fn new(conf: Arc<PaginateConf>) -> Self {
        Self {
            conf,
            base: None,
            next_url: None,
            cursor: None,
            page: 0,
            seen: 0,
            total: None,
            done: false,
        }
    }

    fn request(&mut self, client: &HttpClient, batch: &[Part]) -> Result<PageRequest, CoreError> {
        let base = match &self.base {
            Some(base) => base.clone(),
            None => {
                let raw = client.resolve_url(batch)?;
                let url = Url::parse(&raw).map_err(|e| {
                    CoreError::config(format!(
                        "paginate: invalid URL '{}': {e}",
                        redact_url_str(&raw)
                    ))
                })?;
                self.base = Some(url.clone());
                url
            }
        };
        let mut params: Vec<(String, String)> = self.conf.order_by.iter().cloned().collect();
        let mut headers = Vec::new();
        let url = match &self.conf.mode {
            Mode::Page {
                param,
                start,
                size_param,
                size,
            } => {
                params.push((param.clone(), (start + self.page).to_string()));
                if let (Some(p), Some(s)) = (size_param, size) {
                    params.push((p.clone(), s.to_string()));
                }
                with_params(&base, &params)
            }
            Mode::Offset {
                offset_param,
                limit_param,
                limit,
                start,
            } => {
                params.push((offset_param.clone(), (start + self.seen).to_string()));
                params.push((limit_param.clone(), limit.to_string()));
                with_params(&base, &params)
            }
            Mode::Cursor { to, .. } => {
                if let Some(cursor) = &self.cursor {
                    match to {
                        CursorTarget::Param(p) => params.push((p.clone(), cursor.clone())),
                        CursorTarget::Header(h) => headers.push((h.clone(), cursor.clone())),
                    }
                }
                with_params(&base, &params)
            }
            Mode::Link | Mode::NextUrl { .. } => match &self.next_url {
                Some(next) => next.clone(),
                None => with_params(&base, &params),
            },
        };
        Ok(PageRequest { url, headers })
    }

    fn observe(
        &mut self,
        part: &Part,
        headers: &HeaderMap,
        url: &Url,
        org: &OrgContext,
    ) -> Result<(), CoreError> {
        self.page += 1;
        let page = self.page;
        let conf = self.conf.clone();
        let needs_body = conf.records.is_some()
            || conf.total.is_some()
            || conf.more.is_some()
            || matches!(
                conf.mode,
                Mode::NextUrl { .. }
                    | Mode::Cursor {
                        from: CursorFrom::Path(_),
                        ..
                    }
            );
        let body: Option<JsonValue> =
            if needs_body {
                Some(serde_json::from_slice(&part.as_bytes_ref()).map_err(|e| {
                    CoreError::other(format!("paginate: page {page} is not JSON: {e}"))
                })?)
            } else {
                None
            };
        let count = match &conf.records {
            Some(path) => Some(
                at(&body, path)
                    .and_then(JsonValue::as_array)
                    .map(|a| a.len() as u64)
                    .ok_or_else(|| {
                        CoreError::other(format!(
                            "paginate: page {page} has no array at records path '{path}'"
                        ))
                    })?,
            ),
            None => None,
        };
        self.seen += count.unwrap_or(0);
        if page == 1 {
            self.total = conf
                .total
                .as_deref()
                .and_then(|p| at(&body, p))
                .and_then(as_count);
        }

        let mut done = count == Some(0) || self.total.is_some_and(|t| self.seen >= t);
        if !done {
            if let Some(more) = &conf.more {
                done = !at(&body, more)
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false);
            } else if let (Some(size), Some(n)) = (conf.mode.page_size(), count) {
                done = n < size;
            }
        }
        if !done {
            let base = self.base.clone().unwrap_or_else(|| url.clone());
            match &conf.mode {
                Mode::Cursor { from, .. } => {
                    let next = match from {
                        CursorFrom::Path(path) => at(&body, path).and_then(scalar_text),
                        // A query that fails or answers null names no next cursor: the walk ends.
                        CursorFrom::Query(query) => query
                            .resolve_string(0, &PartsView(std::slice::from_ref(part)))
                            .ok()
                            .filter(|s| !s.is_empty() && s != "null"),
                    };
                    match next {
                        Some(cursor) if self.cursor.as_ref() == Some(&cursor) => {
                            return Err(repeated(page, "cursor"));
                        }
                        Some(cursor) => self.cursor = Some(cursor),
                        None => done = true,
                    }
                }
                Mode::Link => match headers
                    .get_all(reqwest::header::LINK)
                    .iter()
                    .filter_map(|h| h.to_str().ok())
                    .find_map(|h| link_target(h, &["next"]))
                {
                    Some(target) => {
                        self.next_url = Some(not_current(
                            next_target(&base, url, &target, org)?,
                            url,
                            page,
                        )?);
                    }
                    None => done = true,
                },
                Mode::NextUrl { next } => {
                    let target = at(&body, next)
                        .and_then(|v| {
                            v.as_str()
                                .or_else(|| v.get("href").and_then(JsonValue::as_str))
                        })
                        .filter(|s| !s.is_empty());
                    match target {
                        Some(target) => {
                            self.next_url = Some(not_current(
                                next_target(&base, url, target, org)?,
                                url,
                                page,
                            )?);
                        }
                        None => done = true,
                    }
                }
                Mode::Page { .. } | Mode::Offset { .. } => {}
            }
        }

        if let Some(total) = self.total {
            if self.seen > total || (done && self.seen != total) {
                return Err(CoreError::other(format!(
                    "paginate: the source announced {total} records and {} were read; the run is incomplete",
                    self.seen
                )));
            }
        }
        if let Some(max) = conf.max_records {
            if self.seen > max || (!done && self.seen >= max) {
                return Err(CoreError::other(format!(
                    "paginate: max_records ({max}) reached before the stop rule held; the run is incomplete"
                )));
            }
        }
        self.done = done;
        Ok(())
    }
}

/// A source that names the page it just answered as the next one loops: the walk stops at once
/// instead of reading the same page until `max_pages`.
fn repeated(page: u64, what: &str) -> CoreError {
    CoreError::other(format!(
        "paginate: page {page} names the same {what} as the page before; the source loops and the run is incomplete"
    ))
}

fn not_current(next: Url, current: &Url, page: u64) -> Result<Url, CoreError> {
    if next == *current {
        return Err(repeated(page, "next URL"));
    }
    Ok(next)
}

fn at<'a>(body: &'a Option<JsonValue>, path: &str) -> Option<&'a JsonValue> {
    body.as_ref().and_then(|b| lookup(b, path))
}

fn as_count(v: &JsonValue) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn scalar_text(v: &JsonValue) -> Option<String> {
    match v {
        JsonValue::String(s) if !s.is_empty() => Some(s.clone()),
        JsonValue::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The next page of `walk`, or `None` once its stop rule held.
pub async fn next_page(
    client: &HttpClient,
    walk: &mut Walk,
    batch: &[Part],
    org: &OrgContext,
    resources: &ResourceManager,
) -> Result<Option<Part>, CoreError> {
    if walk.done {
        return Ok(None);
    }
    if walk.page >= walk.conf.max_pages as u64 {
        return Err(CoreError::other(format!(
            "paginate: max_pages ({}) reached before the stop rule held; the run is incomplete",
            walk.conf.max_pages
        )));
    }
    if walk.page > 0 && !walk.conf.delay.is_zero() {
        tokio::time::sleep(walk.conf.delay).await;
    }
    let req = walk.request(client, batch)?;
    let (mut out, headers) = client
        .send_page(batch.to_vec(), false, Some(&req), org, resources)
        .await?;
    let Some(mut part) = out.pop() else {
        return Err(CoreError::other("paginate: the page answered no message"));
    };
    walk.observe(&part, &headers, &req.url, org)?;
    part.meta_set("http_page", Value::Int(walk.page as i64));
    Ok(Some(part))
}

/// The `http` processor with `paginate`: each message walks its pages, and each page is a batch
/// of its own; a walk that does not complete flags the message instead of handing on part of it.
pub async fn process_paged(
    client: &HttpClient,
    conf: &Arc<PaginateConf>,
    ctx: &ProcContext,
    batch: Batch,
) -> Result<Vec<Batch>, CoreError> {
    let mut out = Vec::new();
    for part in batch {
        match walk_all(client, conf, &part, ctx).await {
            Ok(pages) => out.extend(pages.into_iter().map(|p| vec![p])),
            Err(
                e @ (CoreError::AccessDenied { .. }
                | CoreError::QuotaExceeded { .. }
                | CoreError::ComponentClosed),
            ) => return Err(e),
            Err(err) => {
                let msg = err.to_string();
                let mut failed = part;
                if let Some(code) = extract_status_code_from_err(&msg) {
                    failed.meta_set("http_status_code", Value::Int(code));
                }
                ctx.on_error(&mut failed, msg);
                out.push(vec![failed]);
            }
        }
    }
    Ok(out)
}

async fn walk_all(
    client: &HttpClient,
    conf: &Arc<PaginateConf>,
    part: &Part,
    ctx: &ProcContext,
) -> Result<Vec<Part>, CoreError> {
    let mut walk = Walk::new(conf.clone());
    let source = std::slice::from_ref(part);
    let mut pages = Vec::new();
    let mut bytes = 0usize;
    loop {
        if ctx.is_shutting_down() {
            return Err(CoreError::ComponentClosed);
        }
        let Some(mut page) = next_page(client, &mut walk, source, &ctx.org, &ctx.resources).await?
        else {
            break;
        };
        // The pages of one walk are held together: the run's memory quota bounds them.
        bytes = bytes.saturating_add(page.size_estimate());
        ctx.org.check_batch_memory(bytes)?;
        let mut out = part.shallow_copy();
        out.set_bytes(page.as_bytes());
        for (k, v) in page.meta_all() {
            out.meta_set(k.clone(), v.clone());
        }
        pages.push(out);
    }
    let count = pages.len() as i64;
    for p in &mut pages {
        p.meta_set("http_page_count", Value::Int(count));
    }
    Ok(pages)
}

/// `base` with each of `set` as a query parameter, replacing one of the same name.
fn with_params(base: &Url, set: &[(String, String)]) -> Url {
    if set.is_empty() {
        return base.clone();
    }
    let kept: Vec<(String, String)> = base
        .query_pairs()
        .filter(|(k, _)| !set.iter().any(|(s, _)| s == k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut url = base.clone();
    url.query_pairs_mut()
        .clear()
        .extend_pairs(kept.iter().chain(set.iter()));
    url
}

/// The next URL `target` (relative to `current`), refused unless it keeps `base`'s scheme, host
/// and port and carries no credentials.
fn next_target(
    base: &Url,
    current: &Url,
    target: &str,
    org: &OrgContext,
) -> Result<Url, CoreError> {
    let next = current.join(target).map_err(|e| {
        CoreError::other(format!(
            "paginate: next URL '{}' does not parse: {e}",
            redact_url_str(target)
        ))
    })?;
    let same = next.scheme() == base.scheme()
        && next.host_str() == base.host_str()
        && next.port_or_known_default() == base.port_or_known_default()
        && next.username().is_empty()
        && next.password().is_none();
    if !same {
        return Err(CoreError::AccessDenied {
            org: org.org_id.0.clone(),
            reason: format!(
                "paginate: next URL '{}' leaves {}://{}:{}; the paging step is refused",
                redact_url_str(next.as_str()),
                base.scheme(),
                base.host_str().unwrap_or_default(),
                base.port_or_known_default().unwrap_or(0)
            ),
        });
    }
    Ok(next)
}

/// The target of the first RFC 8288 `Link` header entry whose `rel` is one of `rels`. A comma
/// inside `<…>` belongs to the URL, not to the list.
// ponytail: a quoted `<` inside a link parameter ends that link's parameters early; no source
// seen writes one. Upgrade path: a quote-aware parameter scanner.
pub fn link_target(header: &str, rels: &[&str]) -> Option<String> {
    let mut rest = header;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let close = after.find('>')?;
        let target = &after[..close];
        let tail = &after[close + 1..];
        let params_end = tail.find('<').unwrap_or(tail.len());
        let matched = tail[..params_end].split(';').any(|param| {
            let param = param.trim().trim_end_matches(',');
            param.split_once('=').is_some_and(|(k, v)| {
                k.trim().eq_ignore_ascii_case("rel")
                    && v.trim()
                        .trim_matches('"')
                        .split_whitespace()
                        .any(|r| rels.iter().any(|want| r.eq_ignore_ascii_case(want)))
            })
        });
        if matched {
            return Some(target.to_string());
        }
        rest = &tail[params_end..];
    }
    None
}

/// A body path: keys joined by `.` (a key holding a dot, `@odata.nextLink`, is matched whole),
/// `[n]` for an index and `[key=value]` for the first element whose `key` is `value`.
pub fn lookup<'a>(value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    if path.is_empty() {
        return Some(value);
    }
    if path.starts_with('[') {
        return bracket(value, path);
    }
    let map = value.as_object()?;
    if let Some(found) = map.get(path) {
        return Some(found);
    }
    for (pos, c) in path.char_indices() {
        if c != '.' && c != '[' {
            continue;
        }
        let Some(inner) = map.get(&path[..pos]) else {
            continue;
        };
        let rest = if c == '.' {
            &path[pos + 1..]
        } else {
            &path[pos..]
        };
        if let Some(found) = lookup(inner, rest) {
            return Some(found);
        }
    }
    None
}

fn bracket<'a>(value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    let close = path.find(']')?;
    let inner = &path[1..close];
    let after = &path[close + 1..];
    let rest = after.strip_prefix('.').unwrap_or(after);
    let list = value.as_array()?;
    let item = match inner.split_once('=') {
        Some((key, want)) => list
            .iter()
            .find(|e| e.get(key).and_then(JsonValue::as_str) == Some(want))?,
        None => list.get(inner.parse::<usize>().ok()?)?,
    };
    lookup(item, rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap as AxHeaders, HeaderValue, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use ekiben_core::{ComponentConfig, OrgId, OrgQuotas, Processor};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn query(uri: &Uri) -> HashMap<String, String> {
        url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect()
    }

    async fn fake<F>(answer: F) -> String
    where
        F: Fn(&Uri, &AxHeaders) -> Response + Clone + Send + Sync + 'static,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let answer = answer.clone();
            async move { answer(req.uri(), req.headers()) }
        });
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    fn org() -> Arc<OrgContext> {
        OrgContext::new(
            OrgId::new("t"),
            OrgQuotas {
                allow_private_ips: true,
                ..OrgQuotas::default()
            },
        )
    }

    fn items(n: usize) -> Vec<JsonValue> {
        (0..n).map(|i| json!({"id": i})).collect()
    }

    fn body(p: &Part) -> JsonValue {
        serde_json::from_slice(&p.as_bytes_ref()).unwrap()
    }

    fn page_server(
        n: usize,
        claimed: usize,
    ) -> impl Fn(&Uri, &AxHeaders) -> Response + Clone + Send + Sync + 'static {
        move |uri: &Uri, _: &AxHeaders| -> Response {
            let q = query(uri);
            let page: usize = q.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
            let size: usize = q.get("size").and_then(|s| s.parse().ok()).unwrap_or(10);
            let slice: Vec<JsonValue> = items(n)
                .into_iter()
                .skip((page - 1) * size)
                .take(size)
                .collect();
            axum::Json(json!({"items": slice, "count": claimed})).into_response()
        }
    }

    async fn walk(base: &str, conf: JsonValue) -> Result<Vec<Part>, CoreError> {
        let conf = PaginateConf::from_json(&conf, &Environment::new())?;
        let client = HttpClient::from_json_config(
            &json!({"url": format!("{base}/items"), "retries": 0}),
            "GET",
            &Environment::new(),
        )?;
        let (org, resources) = (org(), ResourceManager::new());
        let mut walk = Walk::new(conf);
        let mut pages = Vec::new();
        while let Some(p) = next_page(&client, &mut walk, &[], &org, &resources).await? {
            pages.push(p);
        }
        Ok(pages)
    }

    fn counts(pages: &[Part], key: &str) -> Vec<usize> {
        pages
            .iter()
            .map(|p| body(p)[key].as_array().unwrap().len())
            .collect()
    }

    #[tokio::test]
    async fn page_mode_stops_at_a_short_page_an_empty_page_or_the_total() {
        let conf = json!({"mode": "page", "size_param": "size", "size": 10, "records": "items"});
        let base = fake(page_server(25, 25)).await;
        let pages = walk(&base, conf.clone()).await.unwrap();
        assert_eq!(counts(&pages, "items"), [10, 10, 5]);
        assert_eq!(pages[2].meta_get_str("http_page"), "3");

        let base = fake(page_server(20, 20)).await;
        assert_eq!(
            counts(&walk(&base, conf).await.unwrap(), "items"),
            [10, 10, 0]
        );

        let with_total = json!({"mode": "page", "size_param": "size", "size": 10, "records": "items", "total": "count"});
        let base = fake(page_server(20, 20)).await;
        assert_eq!(
            counts(&walk(&base, with_total).await.unwrap(), "items"),
            [10, 10]
        );
    }

    #[tokio::test]
    async fn a_total_that_does_not_match_fails_the_run() {
        let base = fake(page_server(20, 30)).await;
        let err = walk(&base, json!({"mode": "page", "size_param": "size", "size": 10, "records": "items", "total": "count"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("announced 30"), "{err}");
    }

    #[tokio::test]
    async fn offset_mode_moves_by_the_records_a_capped_server_sent() {
        let base = fake(|uri: &Uri, _: &AxHeaders| -> Response {
            let q = query(uri);
            if q.get("orderByFields").map(String::as_str) != Some("OBJECTID") {
                return (StatusCode::BAD_REQUEST, "no order").into_response();
            }
            let offset: usize = q["resultOffset"].parse().unwrap();
            let asked: usize = q["resultRecordCount"].parse().unwrap();
            let slice: Vec<JsonValue> = items(10)
                .into_iter()
                .skip(offset)
                .take(asked.min(4))
                .collect();
            let mut b = json!({"features": slice});
            if offset + asked.min(4) < 10 {
                b["exceededTransferLimit"] = json!(true);
            }
            axum::Json(b).into_response()
        })
        .await;
        let pages = walk(&base, json!({
            "mode": "offset", "offset_param": "resultOffset", "limit_param": "resultRecordCount",
            "limit": 10, "records": "features", "more": "exceededTransferLimit",
            "order_by": {"param": "orderByFields", "value": "OBJECTID"}
        }))
        .await
        .unwrap();
        let ids: Vec<u64> = pages
            .iter()
            .flat_map(|p| {
                body(p)["features"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|f| f["id"].as_u64().unwrap())
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(ids, (0..10).collect::<Vec<u64>>());
    }

    #[tokio::test]
    async fn cursor_mode_follows_the_cursor_until_it_is_null() {
        let base = fake(|uri: &Uri, _: &AxHeaders| -> Response {
            let (data, next) = match query(uri).get("cursor").map(String::as_str) {
                None => (1, json!("c2")),
                Some("c2") => (2, json!("c3")),
                _ => (3, JsonValue::Null),
            };
            axum::Json(json!({"data": [data], "meta": {"next": next}})).into_response()
        })
        .await;
        let pages = walk(&base, json!({"mode": "cursor", "cursor": "meta.next", "cursor_param": "cursor", "records": "data"}))
            .await
            .unwrap();
        assert_eq!(pages.len(), 3);
    }

    #[tokio::test]
    async fn a_cursor_loop_stops_at_max_pages_with_an_error() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let base = fake(move |_: &Uri, h: &AxHeaders| -> Response {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            // a, b, a, b, ...: a loop longer than one page, which only max_pages ends.
            let want = ["b", "a"][n % 2];
            if n > 0 && h.get("x-cursor").and_then(|v| v.to_str().ok()) != Some(want) {
                return (StatusCode::BAD_REQUEST, "wrong cursor").into_response();
            }
            let next = ["a", "b"][n % 2];
            axum::Json(json!({"next": next})).into_response()
        })
        .await;
        let err = walk(&base, json!({"mode": "cursor", "cursor": "next", "cursor_header": "X-Cursor", "max_pages": 3}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("max_pages (3)"), "{err}");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_page_naming_itself_as_next_stops_the_walk_at_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let base = fake(move |_: &Uri, _: &AxHeaders| -> Response {
            seen.fetch_add(1, Ordering::SeqCst);
            axum::Json(json!({"next": "same"})).into_response()
        })
        .await;
        let err = walk(
            &base,
            json!({"mode": "cursor", "cursor": "next", "cursor_param": "c"}),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("same cursor"), "{err}");
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let base = fake(|uri: &Uri, _: &AxHeaders| -> Response {
            axum::Json(json!({"@odata.nextLink": uri.to_string()})).into_response()
        })
        .await;
        let err = walk(
            &base,
            json!({"mode": "next_url", "next": "@odata.nextLink"}),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("same next URL"), "{err}");
    }

    #[tokio::test]
    async fn link_mode_follows_rel_next() {
        let base = fake(|uri: &Uri, _: &AxHeaders| -> Response {
            let p: u64 = query(uri)
                .get("p")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);
            let mut resp = axum::Json(json!({"p": p})).into_response();
            if p < 3 {
                let link = format!(
                    "</items?p=1>; rel=\"first\", </items?p={}>; rel=\"next\"",
                    p + 1
                );
                resp.headers_mut()
                    .insert("link", HeaderValue::from_str(&link).unwrap());
            }
            resp
        })
        .await;
        let pages = walk(&base, json!({"mode": "link"})).await.unwrap();
        let ps: Vec<u64> = pages
            .iter()
            .map(|p| body(p)["p"].as_u64().unwrap())
            .collect();
        assert_eq!(ps, [1, 2, 3]);
    }

    #[tokio::test]
    async fn next_url_mode_follows_odata_next_link() {
        let base = fake(|uri: &Uri, h: &AxHeaders| -> Response {
            let host = h
                .get("host")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let skip: usize = query(uri)
                .get("$skip")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let slice: Vec<JsonValue> = items(5).into_iter().skip(skip).take(2).collect();
            let mut b = json!({"value": slice});
            if skip + 2 < 5 {
                b["@odata.nextLink"] = json!(format!("http://{host}/items?$skip={}", skip + 2));
            }
            axum::Json(b).into_response()
        })
        .await;
        let pages = walk(
            &base,
            json!({"mode": "next_url", "next": "@odata.nextLink", "records": "value"}),
        )
        .await
        .unwrap();
        assert_eq!(counts(&pages, "value"), [2, 2, 1]);
    }

    #[tokio::test]
    async fn a_next_url_on_another_host_or_port_is_refused() {
        for next in ["http://example.org/items", "http://127.0.0.1:1/items"] {
            let base = fake(move |_: &Uri, _: &AxHeaders| -> Response {
                axum::Json(json!({"value": [1], "@odata.nextLink": next})).into_response()
            })
            .await;
            let err = walk(
                &base,
                json!({"mode": "next_url", "next": "@odata.nextLink"}),
            )
            .await
            .unwrap_err();
            assert!(
                matches!(err, CoreError::AccessDenied { .. }),
                "{next}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn the_processor_emits_one_batch_per_page_and_flags_an_incomplete_walk() {
        let ctx = ProcContext {
            stream_id: "s".into(),
            org: org(),
            resources: ResourceManager::new(),
            registry: Arc::new(ekiben_core::Registry::new()),
            blobl_env: Environment::new(),
            metrics: ekiben_core::StreamMetrics::new("s", "t"),
            label: String::new(),
            component_type: "http".into(),
            path: "pipeline.processors.0".into(),
            file_root: None,
            shutdown_rx: tokio::sync::watch::channel(false).1,
        };
        for (claimed, complete) in [(15, true), (99, false)] {
            let base = fake(page_server(15, claimed)).await;
            let conf = ComponentConfig {
                label: String::new(),
                component_type: "http".into(),
                config: json!({"url": format!("{base}/items"), "verb": "GET", "retries": 0,
                    "paginate": {"mode": "page", "size_param": "size", "size": 10, "records": "items", "total": "count"}}),
                processors: Vec::new(),
            };
            let proc = crate::processor::HttpProcessor::from_config(&conf, &ctx).unwrap();
            let out = proc
                .process_batch(&ctx, vec![Part::from_str("{}")])
                .await
                .unwrap();
            if complete {
                assert_eq!(out.len(), 2);
                assert!(out.iter().all(|b| b.len() == 1
                    && !b[0].is_errored()
                    && b[0].meta_get_str("http_page_count") == "2"));
            } else {
                assert_eq!(out.len(), 1);
                assert!(out[0][0].is_errored());
            }
        }
    }

    #[test]
    fn a_config_outside_the_schema_is_refused() {
        let env = Environment::new();
        for bad in [
            json!({}),
            json!({"mode": "scroll"}),
            json!({"mode": "page"}),
            json!({"mode": "page", "records": "a", "max_pages": 0}),
            json!({"mode": "page", "records": "a", "max_pages": 10001}),
            json!({"mode": "page", "records": "a", "limit": 5}),
            json!({"mode": "page", "records": "a", "size_param": "s"}),
            json!({"mode": "offset", "records": "a"}),
            json!({"mode": "cursor", "cursor": "n", "cursor_param": "c", "cursor_header": "X-C"}),
            json!({"mode": "cursor", "cursor_param": "c"}),
            json!({"mode": "next_url"}),
            json!({"mode": "link", "total": "count"}),
            json!({"mode": "link", "order_by": {"param": "o"}}),
            json!({"mode": "link", "delay": "soon"}),
        ] {
            assert!(PaginateConf::from_json(&bad, &env).is_err(), "{bad}");
        }
        assert!(PaginateConf::from_json(&json!({"mode": "link", "delay": "200ms"}), &env).is_ok());
    }

    #[test]
    fn body_paths_and_link_headers_are_read() {
        let v = json!({
            "@odata.nextLink": "n1",
            "links": [{"rel": "self", "href": "s"}, {"rel": "next", "href": "n2"}],
            "_links": {"next": {"href": "n3"}},
            "result": {"results": [1, 2]},
            "a": [[5]]
        });
        assert_eq!(lookup(&v, "@odata.nextLink"), Some(&json!("n1")));
        assert_eq!(lookup(&v, "links[rel=next].href"), Some(&json!("n2")));
        assert_eq!(lookup(&v, "_links.next"), Some(&json!({"href": "n3"})));
        assert_eq!(
            lookup(&v, "result.results")
                .and_then(JsonValue::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(lookup(&v, "a[0][0]"), Some(&json!(5)));
        assert_eq!(lookup(&v, "links[rel=prev].href"), None);
        assert_eq!(lookup(&v, "missing.path"), None);

        let h = "<https://a/x?a=1,2>; rel=\"prev\", <https://a/y>; rel=\"next last\"";
        assert_eq!(link_target(h, &["next"]).as_deref(), Some("https://a/y"));
        assert_eq!(link_target("<https://a/x>; rel=prev", &["next"]), None);
    }
}
