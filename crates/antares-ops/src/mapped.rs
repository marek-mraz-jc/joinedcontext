// SPDX-License-Identifier: EUPL-1.2
//! A view of another model (gap G18, `docs/analysis/view-mapping.md`
//! sections 4 and 5), whichever surface asks ([`ViewOps::run`]): the request
//! inverted into the source model, run through the view as any other, and
//! the answer rebuilt in the target model.

use crate::mapping::{self, Mapping};
use crate::{Caller, OperationId, OpsRequest, Refusal, ViewOps};
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;

/// Why a request that names its own `@context` is refused.
pub const OWN_CONTEXT: &str =
    "a mapped view names its attributes by its own @context; send no Link header";
/// Why a `GeoJSON` answer is refused.
pub const NO_GEOJSON: &str = "a mapped view answers JSON and JSON-LD, not GeoJSON";

/// The method and headers of the request a mapped view serves.
struct Head {
    method: Method,
    headers: HeaderMap,
}

/// Serve `req` as `op` through `view`, whose mapping is `mapping`.
pub(crate) async fn serve(
    view: &ViewOps,
    mapping: &Mapping,
    op: OperationId,
    caller: &Caller,
    req: OpsRequest,
) -> Result<Response, Refusal> {
    let OpsRequest {
        method,
        path_and_query,
        headers,
        body,
        ..
    } = req;
    if headers.contains_key(header::LINK) {
        return Err(Refusal::Malformed(OWN_CONTEXT));
    }
    if headers
        .get_all(header::ACCEPT)
        .iter()
        .any(|v| v.to_str().is_ok_and(|v| v.contains("geo+json")))
    {
        return Err(Refusal::Malformed(NO_GEOJSON));
    }
    let path = path_and_query
        .split_once('?')
        .map_or(path_and_query.as_str(), |(path, _)| path);
    // a query that is no form encoding is refused before a view runs it
    let pairs = path_and_query
        .parse::<axum::http::Uri>()
        .ok()
        .and_then(|uri| axum::extract::Query::<Vec<(String, String)>>::try_from_uri(&uri).ok())
        .map(|q| q.0)
        .ok_or(Refusal::Malformed("the query is not form encoded"))?;
    let parts = Head { method, headers };
    if mapping::DISCOVERY.contains(&op) {
        // every discovery read is a GET: no body
        return discover(view, mapping, op, caller, parts, path, &pairs).await;
    }
    let inverted = mapping.invert(&pairs).map_err(Refusal::Malformed)?;
    if inverted.selects_nothing {
        return if matches!(op, OperationId::QueryEntities | OperationId::TemporalQuery) {
            Ok(empty_page(&parts.headers, &pairs))
        } else {
            Err(Refusal::NotFound)
        };
    }
    // `/entities/{id}/attrs/{attr}[/value]`: the attribute segment
    let mut attribute = None;
    let path = if op == OperationId::RetrieveAttributeValue {
        let mut segments: Vec<String> = path.split('/').map(str::to_owned).collect();
        let at = segments
            .iter()
            .position(|s| s == "attrs")
            .and_then(|i| i.checked_add(1))
            .ok_or(Refusal::NotFound)?;
        let segment = segments.get_mut(at).ok_or(Refusal::NotFound)?;
        let (slot, source) = mapping
            .source_attribute(segment)
            .map_err(Refusal::Malformed)?;
        *segment = source;
        attribute = Some(slot);
        segments.join("/")
    } else {
        path.to_owned()
    };
    let target = if inverted.pairs.is_empty() {
        path
    } else {
        format!("{path}?{}", encode(&inverted.pairs))
    };
    let mut inner = OpsRequest::new(parts.method, target)
        .body(body)
        .context(mapping.inner_context().clone());
    for (name, value) in &parts.headers {
        inner = inner.header(name.clone(), value.clone());
    }
    let history = mapping::HISTORY.contains(&op);
    answer(view, mapping, op, caller, inner, |json| match attribute {
        Some(slot) => mapping.rebuild_attribute(slot, json),
        None if history => {
            mapping.rebuild_history(json, &inverted.keep);
            true
        }
        None => {
            mapping.rebuild(json, &inverted.keep);
            true
        }
    })
    .await
}

/// A discovery read (10.6.3 to 10.6.8): the type or Attribute its path
/// names inverted, the view's narrowed answer named by the mapping.
async fn discover(
    view: &ViewOps,
    mapping: &Mapping,
    op: OperationId,
    caller: &Caller,
    parts: Head,
    path: &str,
    pairs: &[(String, String)],
) -> Result<Response, Refusal> {
    mapping::discovery_pairs(pairs).map_err(Refusal::Malformed)?;
    let (resource, segment) = path.rsplit_once('/').ok_or(Refusal::NotFound)?;
    let mut slot = None;
    let path = match op {
        OperationId::RetrieveEntityTypeInfo => {
            let class = mapping.source_type(segment).ok_or(Refusal::NotFound)?;
            format!("{resource}/{class}")
        }
        OperationId::RetrieveAttributeInfo => {
            let (s, source) = mapping
                .source_attribute(segment)
                .map_err(Refusal::Malformed)?;
            slot = Some(s);
            format!("{resource}/{source}")
        }
        _ => path.to_owned(),
    };
    let target = if pairs.is_empty() {
        path
    } else {
        format!("{path}?{}", encode(pairs))
    };
    let mut inner = OpsRequest::new(parts.method, target).context(mapping.inner_context().clone());
    for (name, value) in &parts.headers {
        inner = inner.header(name.clone(), value.clone());
    }
    answer(view, mapping, op, caller, inner, |json| {
        mapping.rebuild_discovery(json, slot);
        true
    })
    .await
}

/// Run `inner` through the view and `rebuild` its answer in the target
/// model; a refusal is reworded into the target model.
async fn answer(
    view: &ViewOps,
    mapping: &Mapping,
    op: OperationId,
    caller: &Caller,
    inner: OpsRequest,
    rebuild: impl FnOnce(&mut serde_json::Value) -> bool,
) -> Result<Response, Refusal> {
    let answer = view.run_unmapped(op, caller, inner).await?;
    let (mut parts, body) = answer.into_parts();
    let bytes = view.read_answer(body, "mapped").await?;
    parts.headers.remove(header::CONTENT_LENGTH);
    if !parts.status.is_success() {
        // the broker words a refusal in the inner request's terms
        let text = mapping.reword(&String::from_utf8_lossy(&bytes));
        return Ok(Response::from_parts(parts, Body::from(text)));
    }
    // HEAD, 204: no body to rebuild
    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(Response::from_parts(parts, Body::from(bytes.into_bytes())));
    };
    if !rebuild(&mut json) {
        // a value the view cannot translate is not served
        return Err(Refusal::NotFound);
    }
    Ok(Response::from_parts(parts, Body::from(json.to_string())))
}

/// Query Entities whose `type` names nothing the view serves: an empty
/// page, answered without asking the store (section 4.1).
fn empty_page(headers: &HeaderMap, pairs: &[(String, String)]) -> Response {
    let ld = headers
        .get_all(header::ACCEPT)
        .iter()
        .any(|v| v.to_str().is_ok_and(|v| v.contains("application/ld+json")));
    let mut r = Response::new(Body::from("[]"));
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(if ld {
            "application/ld+json"
        } else {
            "application/json"
        }),
    );
    if pairs.iter().any(|(k, v)| k == "count" && v == "true") {
        r.headers_mut()
            .insert("ngsild-results-count", HeaderValue::from_static("0"));
    }
    *r.status_mut() = StatusCode::OK;
    r
}

/// A query string from decoded pairs: every byte outside RFC 3986's
/// unreserved set percent-encoded.
fn encode(pairs: &[(String, String)]) -> String {
    use crate::percent::encode as one;
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", one(k), one(v)))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_encoded_byte_for_byte() {
        let pairs = vec![
            ("q".to_owned(), "a==\"x y\";b>1|c~=\"ž\"".to_owned()),
            ("attrs".to_owned(), "a,b".to_owned()),
        ];
        assert_eq!(
            encode(&pairs),
            "q=a%3D%3D%22x%20y%22%3Bb%3E1%7Cc~%3D%22%C5%BE%22&attrs=a%2Cb"
        );
        let back = axum::extract::Query::<Vec<(String, String)>>::try_from_uri(
            &format!("/e?{}", encode(&pairs)).parse().unwrap_or_default(),
        )
        .map(|q| q.0)
        .unwrap_or_default();
        assert_eq!(back, pairs);
    }
}
