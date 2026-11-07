// SPDX-License-Identifier: EUPL-1.2
//! The feed outputs of one view as Atom (RFC 4287), RSS 2.0 and JSON Feed 1.1:
//! `/x/views/{view}/feed/v1/{name}/{atom.xml|rss.xml|feed.json}` (T-5202, platform API/02 §8b,
//! EP-150, EP-151).
//!
//! A view serves a feed only for an output its `representations.feed` lists; anything else,
//! an unknown view and a view that does not allow Query Entities answer the same 404. The
//! output's sources are read with one NGSI-LD Query Entities through the view
//! ([`antares_ops::ViewOps::scan`]), so the view's narrowing, the Policy and the Tenant decide
//! as for NGSI-LD. The output's `filter` and the request's `q` and `scopeQ` are joined with
//! `;`, and a request text with unbalanced parentheses is `400`, so a request narrows the
//! filter and never widens it. Entries are sorted newest first by `updated`, then by id, and
//! paged by `limit` and `offset` with RFC 5005 links. An Entity without a title or an
//! `updated` time is left out and counted (EP-126). Answers carry `ETag`, `Last-Modified`
//! and `Cache-Control`; a matching conditional GET is `304`. Read only.
//!
//! ```
//! use antares_ops::Surface;
//! assert_eq!(antares_feed::Feed.name(), "feed");
//! assert_eq!(antares_feed::Feed.prefix(), "/x/views");
//! ```

#![forbid(unsafe_code)]

pub mod output;
pub mod write;

use antares_ops::percent::{decoded, encode};
use antares_ops::{
    Caller, OperationId, Ops, Page, Problem, Pushdown, Scanned, Surface, ViewOps, RESIDUAL_SCAN,
};
use axum::extract::{Query, Request};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use axum::http::{HeaderMap, HeaderName, StatusCode, Uri};
use axum::response::{IntoResponse as _, Response};
use axum::routing::any;
use axum::Router;
use chrono::{DateTime, Utc};
use output::{local, Entry, Plan, MAX_LIMIT, MAX_OUTPUTS};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::hash::{Hash as _, Hasher as _};
use write::{Format, Head};

/// Where the broker mounts the surface; the view, `feed/v1`, the output and the file follow.
pub const PREFIX: &str = "/x/views";

/// The metric an Entity left out of a feed increments (EP-126).
pub const SKIPPED: &str = "antares_surface_records_skipped_total";

/// The header a feed reports how many Entities it left out in.
pub const SKIPPED_HEADER: &str = "antares-skipped-records";

/// How long the reader alone may keep a page, in seconds: `private`, since the Policy decides
/// per caller and a shared cache would hand one subject's entries to another.
pub const MAX_AGE: u32 = 300;

/// The Query Entities parameters a feed passes through as they are (TS 104 175 6.4.3.2);
/// `q`, `scopeQ` and `lang` are joined with the output's own.
const GEO: [&str; 4] = ["georel", "geometry", "coordinates", "geoproperty"];

/// The year of every tag URI (RFC 4151 2.1): fixed, so an id never changes.
const TAG_YEAR: &str = "2026";

/// The feed surface.
#[derive(Debug, Clone, Copy, Default)]
pub struct Feed;

impl Surface for Feed {
    fn name(&self) -> &'static str {
        "feed"
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn version_info(&self) -> Value {
        json!({
            "endpoint": format!("{PREFIX}/{{view}}/feed/v1/{{name}}/{{atom.xml|rss.xml|feed.json}}"),
            "conformsTo": [
                "https://www.rfc-editor.org/rfc/rfc4287",
                "https://www.rssboard.org/rss-specification",
                "https://www.jsonfeed.org/version/1.1/",
                "https://www.rfc-editor.org/rfc/rfc5005",
            ],
        })
    }

    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{view}/feed/v1/{name}/{file}",
            any(move |caller: Caller, req: Request| {
                let ops = ops.clone();
                async move { serve(&ops, &caller, req).await }
            }),
        )
    }
}

fn not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "no such feed")
}

fn bad(title: impl Into<String>) -> Problem {
    Problem::new(StatusCode::BAD_REQUEST, title)
}

async fn serve(ops: &Ops, caller: &Caller, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let refused =
        || Problem::new(StatusCode::METHOD_NOT_ALLOWED, "this surface is read only").reply();
    antares_ops::read_only(&parts.method, refused, async {
        match page(ops, caller, &parts.uri, &parts.headers).await {
            Ok(served) => served.reply(&parts.headers),
            Err(problem) => problem.reply(),
        }
    })
    .await
}

/// A page rendered, with what its validators and its skip count say.
struct Served {
    format: Format,
    body: String,
    updated: DateTime<Utc>,
    skipped: usize,
}

impl Served {
    /// `200` with the page, or `304` when the request's validator still holds (RFC 9110 13.2.2:
    /// `If-None-Match` first, `If-Modified-Since` only without it).
    fn reply(self, headers: &HeaderMap) -> Response {
        let mut hasher = std::hash::DefaultHasher::new();
        self.body.hash(&mut hasher);
        let etag = format!("\"{:016x}-{:x}\"", hasher.finish(), self.body.len());
        let modified = self.updated.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
        let fresh = match (header(IF_NONE_MATCH), header(IF_MODIFIED_SINCE)) {
            (Some(tags), _) => tags
                .split(',')
                .map(|t| t.trim().trim_start_matches("W/"))
                .any(|t| t == etag || t == "*"),
            (None, Some(since)) => DateTime::parse_from_rfc2822(since)
                .is_ok_and(|since| self.updated.timestamp() <= since.timestamp()),
            (None, None) => false,
        };
        let validators = [
            (ETAG, etag),
            (LAST_MODIFIED, modified),
            (CACHE_CONTROL, format!("private, max-age={MAX_AGE}")),
        ];
        if fresh {
            return (StatusCode::NOT_MODIFIED, validators).into_response();
        }
        (
            StatusCode::OK,
            validators,
            [
                (CONTENT_TYPE, self.format.media_type().to_owned()),
                (
                    HeaderName::from_static(SKIPPED_HEADER),
                    self.skipped.to_string(),
                ),
            ],
            self.body,
        )
            .into_response()
    }
}

/// `/{view}/feed/v1/{name}/{file}`: a view id and an output name are `[a-z0-9-]`.
fn segments(uri: &Uri) -> Option<(String, String, Format)> {
    let mut parts = uri.path().split('/').skip(1);
    let view = decoded(parts.next()?)?;
    let (_, _, name, file) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    Some((view, decoded(name)?, Format::of(file)?))
}

/// The output named `name` of the view, compiled. The list is checked whole (at most
/// [`MAX_OUTPUTS`], names unique) so a list the gateway should have refused serves nobody;
/// another output that does not compile does not stop this one.
fn plan(view: &ViewOps, name: &str) -> Result<Plan, Problem> {
    let broken = || {
        Problem::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the view's feed outputs do not hold; its Endpoint must be published again",
        )
    };
    let Some(written) = view.representation("feed") else {
        return Err(not_found());
    };
    let outputs = written.as_array().ok_or_else(broken)?;
    let names: BTreeSet<&str> = outputs
        .iter()
        .filter_map(|o| o.get("name").and_then(Value::as_str))
        .collect();
    if outputs.len() > MAX_OUTPUTS || names.len() != outputs.len() {
        return Err(broken());
    }
    let written = outputs
        .iter()
        .find(|o| o.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(not_found)?;
    Plan::of(written).map(|(p, _)| p).map_err(|_| broken())
}

/// A request's query string read.
#[derive(Debug, Default)]
struct Asked {
    pairs: Vec<(String, String)>,
    seen: BTreeMap<String, String>,
    limit: Option<usize>,
    offset: usize,
}

fn asked(uri: &Uri) -> Result<Asked, Problem> {
    // a list of pairs reads any query string (lossy UTF-8): this never fails
    let pairs = Query::<Vec<(String, String)>>::try_from_uri(uri)
        .map(|Query(p)| p)
        .unwrap_or_default();
    let mut seen = BTreeMap::new();
    for (key, value) in &pairs {
        let known = ["q", "scopeQ", "lang", "attrs", "type", "limit", "offset"];
        if !known.contains(&key.as_str()) && !GEO.contains(&key.as_str()) {
            return Err(bad(format!(
                "{key:?} is not a parameter of this feed: q, scopeQ, {}, lang, attrs, type, limit, offset",
                GEO.join(", ")
            )));
        }
        if seen.insert(key.clone(), value.clone()).is_some() {
            return Err(bad(format!("{key} is given more than once")));
        }
    }
    let number = |name: &str| {
        seen.get(name)
            .map(|v| {
                v.parse::<usize>()
                    .map_err(|_| bad("limit and offset take a whole number"))
            })
            .transpose()
    };
    let limit = number("limit")?;
    if limit.is_some_and(|l| !(1..=MAX_LIMIT).contains(&l)) {
        return Err(bad(format!("limit is 1 to {MAX_LIMIT}")));
    }
    let offset = number("offset")?.unwrap_or(0);
    Ok(Asked {
        pairs,
        seen,
        limit,
        offset,
    })
}

/// Whether the parentheses of a query text balance outside its quoted strings, so the text
/// joined after `;` can never close the output's own group.
fn balanced(text: &str) -> bool {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for c in text.chars() {
        match (c, quoted) {
            (_, true) if escaped => escaped = false,
            ('\\', true) => escaped = true,
            ('"', _) => quoted = !quoted,
            ('(', false) => depth = depth.saturating_add(1),
            (')', false) => match depth.checked_sub(1) {
                Some(d) => depth = d,
                None => return false,
            },
            _ => {}
        }
    }
    depth == 0 && !quoted
}

/// The output's text and the request's, joined so both hold.
fn joined(name: &str, own: Option<&str>, asked: Option<&str>) -> Result<Option<String>, Problem> {
    if asked.is_some_and(|a| !balanced(a)) {
        return Err(bad(format!("{name} has unbalanced parentheses or quotes")));
    }
    Ok(match (own, asked) {
        (Some(o), Some(a)) => Some(format!("({o});({a})")),
        (o, a) => o.or(a).map(str::to_owned),
    })
}

/// The Query Entities a page reads: the view's types the output lists (narrowed by a
/// request's `type`), the joined `q` and `scopeQ`, one geo query and the language.
async fn pushdown(
    view: &ViewOps,
    plan: &Plan,
    asked: &Asked,
) -> Result<(Pushdown, Vec<String>), Problem> {
    let mut wanted: Vec<String> = plan.types().map(str::to_owned).collect();
    if let Some(types) = asked.seen.get("type") {
        let named: Vec<&str> = types.split(',').map(local).collect();
        if named.iter().any(|t| !wanted.iter().any(|w| w == t)) {
            return Err(bad(format!("type is one of {}", wanted.join(", "))));
        }
        wanted.retain(|w| named.contains(&w.as_str()));
    }
    let wanted_names: Vec<&str> = wanted.iter().map(String::as_str).collect();
    let mut query = view
        .type_selector(&wanted_names)
        .await
        .ok_or_else(not_found)?;
    let filter = &plan.filter;
    let mut add = |key: &str, value: Option<String>| {
        if let Some(v) = value {
            let _ = write!(query, "&{key}={}", encode(&v));
        }
    };
    let get = |k: &str| asked.seen.get(k).map(String::as_str);
    add("q", joined("q", filter.q.as_deref(), get("q"))?);
    add(
        "scopeQ",
        joined("scopeQ", filter.scope_q.as_deref(), get("scopeQ"))?,
    );
    let lang = get("lang").map(str::to_owned).or_else(|| plan.lang.clone());
    add("lang", lang);
    let geo_asked = GEO.iter().any(|k| asked.seen.contains_key(*k));
    if output::fixes_area(filter) && geo_asked {
        return Err(bad(
            "this feed fixes its area: leave out georel, geometry and coordinates",
        ));
    }
    let own = [
        ("georel", &filter.georel),
        ("geometry", &filter.geometry),
        ("coordinates", &filter.coordinates),
        ("geoproperty", &filter.geoproperty),
    ];
    for (key, value) in own {
        add(key, value.clone().or_else(|| get(key).map(str::to_owned)));
    }
    let attrs = asked
        .seen
        .get("attrs")
        .map(|a| a.split(',').map(str::to_owned).collect());
    let pushdown = Pushdown {
        query,
        attrs,
        sys_attrs: true,
        ..Pushdown::default()
    };
    Ok((pushdown, wanted))
}

/// Every entry of the output the caller reads, newest first, and how many were left out.
async fn entries(
    view: &ViewOps,
    caller: &Caller,
    plan: &Plan,
    asked: &Asked,
) -> Result<(Vec<Entry>, usize), Problem> {
    let (pushdown, wanted) = pushdown(view, plan, asked).await?;
    let lang = asked
        .seen
        .get("lang")
        .or(plan.lang.as_ref())
        .map(String::as_str);
    // ponytail: the order is applied after the read, so a feed reads the whole answer, which the
    // scan refuses past RESIDUAL_SCAN Entities; a caller narrows with q past that.
    let whole = Page {
        start: 0,
        size: RESIDUAL_SCAN,
        whole: true,
    };
    let entities = match view.scan(caller, &pushdown, None, whole).await {
        Ok(Scanned::Page { entities, .. }) => entities,
        // A Tenant that holds nothing yet answers 404 (5.5.10): an empty feed.
        Ok(Scanned::Answer(a)) if a.status() == StatusCode::NOT_FOUND => Vec::new(),
        Ok(Scanned::Answer(a)) => return Err(Problem::Answer(Box::new(a))),
        Err(refusal) => return Err(Problem::Refused(refusal)),
    };
    let (mut out, mut skipped) = (Vec::new(), 0usize);
    for entity in &entities {
        // a view of every type reads them all: only the types asked for are entries
        let ty = local(
            entity
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        let listed = wanted.iter().any(|w| w == ty);
        match plan.entry(entity, lang).filter(|_| listed) {
            Some(Ok(entry)) => out.push(entry),
            Some(Err(reason)) => {
                skipped = skipped.saturating_add(1);
                metrics::counter!(SKIPPED, "surface" => "feed", "reason" => reason).increment(1);
            }
            None => {}
        }
    }
    out.sort_by(|a, b| (b.updated, &a.entity).cmp(&(a.updated, &b.entity)));
    Ok((out, skipped))
}

/// The host of a base URL, without scheme, port or path: a tag URI's authority.
fn host(base: &str) -> &str {
    let rest = base.split_once("://").map_or(base, |(_, r)| r);
    let authority = rest.split('/').next().unwrap_or_default();
    authority.rsplit_once(':').map_or(authority, |(h, _)| h)
}

/// The page's URL with `offset` set (none: the first page), the other parameters kept.
fn url_at(this: &str, pairs: &[(String, String)], offset: Option<usize>) -> String {
    let mut query: Vec<String> = pairs
        .iter()
        .filter(|(k, _)| k != "offset")
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect();
    if let Some(o) = offset {
        query.push(format!("offset={o}"));
    }
    if query.is_empty() {
        this.to_owned()
    } else {
        format!("{this}?{}", query.join("&"))
    }
}

/// The text of `texts` in `lang`, else its first.
fn in_lang(texts: &BTreeMap<String, String>, lang: Option<&str>) -> Option<String> {
    lang.and_then(|l| texts.get(l))
        .or_else(|| texts.values().next())
        .cloned()
}

async fn page(
    ops: &Ops,
    caller: &Caller,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<Served, Problem> {
    let (id, name, format) = segments(uri).ok_or_else(not_found)?;
    // a view that does not allow Query Entities has no feed, as an unknown one
    let view = ops
        .view(&id)
        .filter(|v| v.allows(OperationId::QueryEntities))
        .ok_or_else(not_found)?;
    let plan = plan(&view, &name)?;
    let asked = asked(uri)?;
    let (all, skipped) = entries(&view, caller, &plan, &asked).await?;
    let limit = asked.limit.unwrap_or(plan.limit);
    let shown: Vec<Entry> = all.iter().skip(asked.offset).take(limit).cloned().collect();
    let base = antares_ops::public_base(headers);
    let host = host(&base);
    let this = format!("{base}{PREFIX}{}", uri.path());
    let after = asked.offset.saturating_add(limit);
    let lang = asked.seen.get("lang").or(plan.lang.as_ref()).cloned();
    let head = Head {
        id: format!("tag:{host},{TAG_YEAR}:{id}/feed/{name}"),
        title: in_lang(&plan.title, lang.as_deref()).unwrap_or_else(|| name.clone()),
        description: in_lang(&plan.description, lang.as_deref()),
        lang,
        updated: all.first().map_or(DateTime::UNIX_EPOCH, |e| e.updated),
        this: uri
            .query()
            .map_or_else(|| this.clone(), |q| format!("{this}?{q}")),
        first: url_at(&this, &asked.pairs, None),
        previous: (asked.offset > 0).then(|| {
            url_at(
                &this,
                &asked.pairs,
                Some(asked.offset.saturating_sub(limit)),
            )
        }),
        next: (after < all.len()).then(|| url_at(&this, &asked.pairs, Some(after))),
        author: host.to_owned(),
        entry_ids: format!("tag:{host},{TAG_YEAR}:{id}/"),
    };
    Ok(Served {
        format,
        body: write::render(format, &head, &shown),
        updated: head.updated,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_text_must_balance_outside_its_strings() {
        for ok in [
            "a==1",
            "(a==1|b==2);c==3",
            r#"name=="x)""#,
            r#"name=="a\"(""#,
        ] {
            assert!(balanced(ok), "{ok}");
        }
        for bad in ["a==1)|(b==2", "(a==1", r#"name=="x"#, ")("] {
            assert!(!balanced(bad), "{bad}");
        }
    }

    #[test]
    fn a_tag_authority_is_the_bare_host() {
        assert_eq!(
            host("https://api.dev.joinedcontext.com"),
            "api.dev.joinedcontext.com"
        );
        assert_eq!(host("http://localhost:9090/x"), "localhost");
        assert_eq!(host("broker"), "broker");
    }
}
