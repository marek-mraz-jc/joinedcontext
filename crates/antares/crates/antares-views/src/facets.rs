// SPDX-License-Identifier: EUPL-1.2
//! `GET /x/views/{view}/facets?attr=…` (T-6757, EP-178): the distinct values of one Attribute
//! among the Entities a Query Entities through the view selects, with how many Entities hold
//! each, under the reader's Policy.
//!
//! The values are counted on the Entities [`ViewOps::scan`] answers for the caller, the same
//! read every surface makes (ADR-0028): the view's rules and the engine's row and Attribute
//! narrowing apply before a value is seen, so an Entity the Policy hides counts nowhere and a
//! value only hidden Entities hold never appears. `attr` must be one of the view's
//! `representations.facets`, checked against the closed list and never spliced into a query;
//! a name that is not listed, unknown, or hidden from this caller by the engine answers the
//! same 400. The read is bounded by [`RESIDUAL_SCAN`]: past it the answer is 413, never a count
//! of a part. Without `attr` the answer is the closed list itself, `{"facets": [...]}`, so a
//! page builds its filters from the publisher's attributes (T-6758).
//!
//! Decided (worker-t6757, owner may override): counted in the broker over the scan, not by a
//! SQL `GROUP BY`, because a Policy's row formulas are only partly pushed into SQL (a regex
//! `idPattern`, a geo residual and per-rule Attribute lists are judged in the broker), and a
//! `GROUP BY` beside that path would count what the reader cannot see; options: a store
//! `GROUP BY` used only when the store decided the whole filter (a second count path to keep
//! equal to Query Entities). The view's `representations.facets` stands for the Endpoint's
//! `spec.facets`.

use antares_ops::percent::{decoded, encode};
use antares_ops::views::facet_path;
use antares_ops::{
    Caller, OperationId, Ops, Page, Problem, Pushdown, Scanned, ViewOps, RESIDUAL_SCAN,
};
use axum::extract::{Query, Request};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, VARY};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse as _, Response};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// The route, below [`crate::PREFIX`].
pub const ROUTE: &str = "/{view}/facets";

/// The most values one answer lists.
pub const MAX_LIMIT: usize = 100;

/// The values an answer lists when the caller names no `limit`.
pub const DEFAULT_LIMIT: usize = 20;

/// How long a shared cache may keep an anonymous answer, in seconds.
pub const SHARED_MAX_AGE: u32 = 300;

/// The Query Entities parameters a facet read passes through as they are (TS 104 175 6.4.3.2).
pub const PASSED: [&str; 8] = [
    "q",
    "scopeQ",
    "georel",
    "geometry",
    "coordinates",
    "geoproperty",
    "id",
    "idPattern",
];

/// The one 400 for an `attr` that is not listed, unknown or hidden: the answer tells none of
/// them apart.
const NOT_FACETABLE: &str = "attr is not a facet of this view";

/// The facet read of `req`.
pub(crate) async fn serve(ops: &Ops, caller: &Caller, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let refused = || Problem::new(StatusCode::METHOD_NOT_ALLOWED, "facets are read only").reply();
    antares_ops::read_only(&parts.method, refused, async {
        let id = parts.uri.path().split('/').nth(1).unwrap_or_default();
        match facets(ops, caller, id, &parts.uri).await {
            Ok(body) => answer(caller, &body),
            Err(problem) => problem.reply(),
        }
    })
    .await
}

fn answer(caller: &Caller, body: &Value) -> Response {
    let cache = if caller.is_anonymous() {
        format!("public, max-age={SHARED_MAX_AGE}")
    } else {
        "private, no-store".to_owned()
    };
    let mut reply = (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response();
    let h = reply.headers_mut();
    // a fixed ASCII text is always a header value; the fallback is never sent
    h.insert(
        CACHE_CONTROL,
        HeaderValue::from_str(&cache).unwrap_or(HeaderValue::from_static("no-store")),
    );
    // header names are visible ASCII, so the list is a header value; the fallback is never sent
    h.insert(
        VARY,
        HeaderValue::from_str(&vary(&antares_ops::policy::SUBJECT_HEADERS))
            .unwrap_or(HeaderValue::from_static("*")),
    );
    reply
}

/// The request headers an answer differs by: the credentials, the tenant and every policy
/// subject header, so no cache hands one reader's answer to another.
fn vary(subject: &[String]) -> String {
    ["Authorization", "Cookie", "NGSILD-Tenant"]
        .into_iter()
        .chain(subject.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(", ")
}

fn bad(title: impl Into<String>) -> Problem {
    Problem::new(StatusCode::BAD_REQUEST, title)
}

fn not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "no such view")
}

/// A facet read's query string.
#[derive(Debug)]
struct Asked {
    /// `None`: the answer lists the view's facets instead of counting one.
    attr: Option<String>,
    limit: usize,
    types: Vec<String>,
    passed: BTreeMap<String, String>,
}

fn asked(uri: &axum::http::Uri) -> Result<Asked, Problem> {
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
    let attr = seen.get("attr").map(|a| (*a).to_owned());
    let limit = match seen.get("limit") {
        None => DEFAULT_LIMIT,
        Some(v) => v
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MAX_LIMIT).contains(n))
            .ok_or_else(|| bad(format!("limit takes 1 to {MAX_LIMIT}, e.g. limit=20")))?,
    };
    let mut out = Asked {
        attr,
        limit,
        types: Vec::new(),
        passed: BTreeMap::new(),
    };
    for (key, value) in seen {
        match key {
            "attr" | "limit" => {}
            "type" => out.types = value.split(',').map(str::to_owned).collect(),
            k if PASSED.contains(&k) => {
                out.passed.insert(k.to_owned(), value.to_owned());
            }
            other => {
                return Err(bad(format!(
                    "{other:?} is not a parameter of facets: attr, limit, type, {}",
                    PASSED.join(", ")
                )))
            }
        }
    }
    Ok(out)
}

/// The last segment of a type IRI, or the name itself.
fn local(name: &str) -> &str {
    name.rsplit(['/', '#']).next().unwrap_or(name)
}

/// The answer to a facet read of view `id`.
async fn facets(
    ops: &Ops,
    caller: &Caller,
    id: &str,
    uri: &axum::http::Uri,
) -> Result<Value, Problem> {
    let asked = asked(uri)?;
    let view = decoded(id)
        .and_then(|id| ops.view(&id))
        .filter(|v| v.allows(OperationId::QueryEntities))
        .ok_or_else(not_found)?;
    let facets = view
        .representation("facets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // without `attr` the answer names the closed list, so a page builds its filters from it
    let Some(attr) = asked.attr.as_deref() else {
        return Ok(json!({ "facets": facets }));
    };
    let listed = facets.iter().any(|f| f.as_str() == Some(attr));
    let (name, member) = facet_path(attr)
        .filter(|_| listed)
        .ok_or_else(|| bad(NOT_FACETABLE))?;
    let attr_iri = view.expand_name(name);
    let types = selected_types(&view, &asked.types)?;
    // the engine's answer for this caller: an Attribute it hides is the same 400
    let visible = view
        .visible(
            caller,
            OperationId::QueryEntities,
            &types,
            std::slice::from_ref(&attr_iri),
        )
        .await
        .map_err(Problem::Refused)?;
    if !types
        .iter()
        .any(|t| visible.shows_type(t) && visible.shows_attribute(t, &attr_iri))
    {
        return Err(bad(NOT_FACETABLE));
    }
    let wanted: Vec<&str> = types.iter().map(|t| local(t)).collect();
    let selector = view.type_selector(&wanted).await.ok_or_else(not_found)?;
    let mut query = selector;
    for (k, v) in &asked.passed {
        let _ = write!(query, "&{k}={}", encode(v));
    }
    let pushdown = Pushdown {
        query,
        attrs: Some(vec![attr_iri.clone()]),
        ..Pushdown::default()
    };
    let page = Page {
        start: 0,
        size: RESIDUAL_SCAN,
        whole: true,
    };
    let entities = match view.scan(caller, &pushdown, None, page).await {
        Ok(Scanned::Page {
            entities,
            count,
            truncated,
            ..
        }) => {
            if truncated || count.is_some_and(|n| n > entities.len()) {
                return Err(Problem::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!(
                        "the query selects more than the {RESIDUAL_SCAN} entities one facet read counts: narrow it with q or type"
                    ),
                ));
            }
            entities
        }
        // A Tenant that holds nothing yet answers 404 (5.5.10): no values.
        Ok(Scanned::Answer(a)) if a.status() == StatusCode::NOT_FOUND => Vec::new(),
        Ok(Scanned::Answer(a)) => return Err(Problem::Answer(Box::new(a))),
        Err(refusal) => return Err(Problem::Refused(refusal)),
    };
    let key = antares_ops::core_name(&attr_iri);
    let (values, truncated) = counted(
        entities.iter().filter_map(|e| e.get(&key)),
        member,
        asked.limit,
    );
    Ok(json!({ "attr": attr, "values": values, "truncated": truncated }))
}

/// The expanded types a facet read counts: the caller's `type`, each one the view shows, or
/// every type the view shows. A view of every type needs the caller's `type`.
fn selected_types(view: &ViewOps, asked: &[String]) -> Result<Vec<String>, Problem> {
    if asked.is_empty() {
        if view.shows_every_name() {
            return Err(bad("type is required for this view, e.g. type=Event"));
        }
        return Ok(view
            .shown_names()
            .0
            .iter()
            .map(|t| antares_ops::core_iri(t))
            .collect());
    }
    let shown: BTreeSet<String> = view
        .shown_names()
        .0
        .iter()
        .map(|t| antares_ops::core_iri(t))
        .collect();
    asked
        .iter()
        .map(|t| {
            let iri = view.expand_name(t);
            (view.shows_every_name() || shown.contains(&iri))
                .then_some(iri)
                .ok_or_else(|| bad(format!("this view does not show the type {t:?}")))
        })
        .collect()
}

/// The `limit` values most Entities hold, ties in value order, and whether more exist. Each
/// Entity counts a value once, however many instances or list items hold it.
fn counted<'a>(
    attrs: impl Iterator<Item = &'a Value>,
    member: Option<&str>,
    limit: usize,
) -> (Vec<Value>, bool) {
    let mut counts: BTreeMap<String, (Value, u64)> = BTreeMap::new();
    for attr in attrs {
        let mut held = BTreeMap::new();
        for value in values_of(attr, member) {
            held.entry(value.to_string()).or_insert(value);
        }
        for (k, v) in held {
            let n = &mut counts.entry(k).or_insert((v, 0)).1;
            *n = n.saturating_add(1);
        }
    }
    let truncated = counts.len() > limit;
    let mut ranked: Vec<(Value, u64)> = counts.into_values().collect();
    // stable: equal counts keep the value order of the map
    ranked.sort_by_key(|a| std::cmp::Reverse(a.1));
    let values = ranked
        .into_iter()
        .take(limit)
        .map(|(value, count)| json!({ "value": value, "count": count }))
        .collect();
    (values, truncated)
}

/// The scalar values one Attribute holds: of each instance, its `value`, `object` or `vocab`
/// (or `member` of that object), each item of a list; a language map or a structure is no
/// facet value.
fn values_of(attr: &Value, member: Option<&str>) -> Vec<Value> {
    let instances = match attr {
        Value::Array(all) => all.iter().collect(),
        one => vec![one],
    };
    let mut out = Vec::new();
    for instance in instances {
        let held = match instance {
            Value::Object(o) => ["value", "object", "vocab"]
                .iter()
                .find_map(|k| o.get(*k))
                .and_then(|v| match member {
                    Some(m) => v.get(m),
                    None => Some(v),
                }),
            _ => None,
        };
        match held {
            Some(Value::Array(items)) => out.extend(items.iter().filter(|i| scalar(i)).cloned()),
            Some(v) if scalar(v) => out.push(v.clone()),
            _ => {}
        }
    }
    out
}

fn scalar(v: &Value) -> bool {
    matches!(v, Value::String(_) | Value::Number(_) | Value::Bool(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn town(t: &str) -> Value {
        json!({"type": "Property", "value": {"addressLocality": t}})
    }

    #[test]
    fn each_entity_counts_a_value_once_and_the_top_values_win() {
        let attrs = [
            json!({"type": "Property", "value": ["music", "music", "film"]}),
            json!([{"type": "Property", "value": "music", "datasetId": "urn:a"},
                   {"type": "Property", "value": "theatre", "datasetId": "urn:b"}]),
            json!({"type": "Relationship", "object": "urn:ngsi-ld:Place:1"}),
            json!({"type": "LanguageProperty", "languageMap": {"sk": "hudba"}}),
            json!({"type": "Property", "value": {"nested": "x"}}),
        ];
        let (values, truncated) = counted(attrs.iter(), None, 2);
        assert_eq!(
            values,
            vec![
                json!({"value": "music", "count": 2}),
                json!({"value": "film", "count": 1})
            ]
        );
        assert!(truncated);
        let (all, more) = counted(attrs.iter(), None, MAX_LIMIT);
        assert_eq!(all.len(), 4);
        assert!(!more);
    }

    #[test]
    fn a_member_path_reads_one_member_of_the_value() {
        let attrs = [town("Zvolen"), town("Sliač"), town("Zvolen")];
        let (values, truncated) = counted(attrs.iter(), Some("addressLocality"), 20);
        assert_eq!(
            values,
            vec![
                json!({"value": "Zvolen", "count": 2}),
                json!({"value": "Sliač", "count": 1})
            ]
        );
        assert!(!truncated);
    }

    #[test]
    fn the_answer_varies_by_every_policy_subject_header() {
        let subject = ["x-jc-subject".to_owned(), "x-jc-groups".to_owned()];
        assert_eq!(
            vary(&subject),
            "Authorization, Cookie, NGSILD-Tenant, x-jc-subject, x-jc-groups"
        );
        assert_eq!(vary(&[]), "Authorization, Cookie, NGSILD-Tenant");
    }

    #[test]
    fn the_query_string_is_checked_before_anything_runs() {
        let read = |q: &str| {
            format!("/v/facets?{q}")
                .parse::<axum::http::Uri>()
                .ok()
                .and_then(|uri| asked(&uri).ok())
        };
        assert!(read("attr=category&limit=20&type=Event&q=a%3D%3D1").is_some());
        assert!(
            read("").is_some_and(|a| a.attr.is_none()),
            "no attr lists the facets"
        );
        for q in [
            "attr=a&attr=b",
            "attr=a&limit=0",
            "attr=a&limit=101",
            "attr=a&limit=x",
            "attr=a&orderBy=a",
            "attr=a&count=true",
        ] {
            assert!(read(q).is_none(), "{q}");
        }
    }
}
