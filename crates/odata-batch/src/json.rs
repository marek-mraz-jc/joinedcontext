// SPDX-License-Identifier: EUPL-1.2
//! JSON batches (JSON Format 4.01 section 19.1).

use serde_json::{Map, Value};

use crate::{
    check_member, header, is_json, json_depth, Batch, BatchError, BatchLimit, Feature, Limits,
    Method, Request,
};

/// The members of a request object this reader knows; instance annotations (`@...`) are
/// ignored, any other member is [`BatchError::Malformed`].
const MEMBERS: [&str; 6] = ["id", "method", "url", "headers", "body", "dependsOn"];

fn text(value: Option<&Value>) -> Result<String, BatchError> {
    value
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(BatchError::Malformed)
}

fn member(
    object: &Map<String, Value>,
    earlier: &[Request],
    limits: &Limits,
) -> Result<Request, BatchError> {
    if object.contains_key("atomicityGroup") {
        return Err(BatchError::NotImplemented(Feature::AtomicityGroup));
    }
    if object
        .keys()
        .any(|k| !k.starts_with('@') && !MEMBERS.contains(&k.as_str()))
    {
        return Err(BatchError::Malformed);
    }
    let token = text(object.get("method"))?;
    let method = match Method::parse(&token) {
        Some(method) => method,
        None if !token.is_empty() && token.bytes().all(|b| b.is_ascii_alphabetic()) => {
            return Err(BatchError::NotImplemented(Feature::Method));
        }
        None => return Err(BatchError::Malformed),
    };
    let headers = match object.get("headers") {
        None => Vec::new(),
        Some(Value::Object(headers)) => headers
            .iter()
            .map(|(name, value)| value.as_str().map(|v| (name.clone(), v.to_owned())))
            .collect::<Option<Vec<_>>>()
            .ok_or(BatchError::Malformed)?,
        Some(_) => return Err(BatchError::Malformed),
    };
    let depends_on = match object.get("dependsOn") {
        None => Vec::new(),
        Some(Value::Array(ids)) if ids.len() > limits.max_depends_on => {
            return Err(BatchError::limit(
                BatchLimit::DependsOn,
                limits.max_depends_on,
            ));
        }
        Some(Value::Array(ids)) => ids
            .iter()
            .map(|id| {
                let id = id.as_str().ok_or(BatchError::Malformed)?;
                earlier
                    .iter()
                    .position(|e| e.id.as_deref() == Some(id))
                    .ok_or(BatchError::UnknownDependency)
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(BatchError::Malformed),
    };
    let content_type = header(&headers, "Content-Type");
    let body = match (object.get("body"), content_type) {
        (None, _) => Vec::new(),
        (Some(body), None) => body.to_string().into_bytes(),
        (Some(body), Some(t)) if is_json(t) => body.to_string().into_bytes(),
        (Some(Value::String(text)), Some(t))
            if t.trim_start().to_ascii_lowercase().starts_with("text/") =>
        {
            text.clone().into_bytes()
        }
        (Some(_), Some(_)) => return Err(BatchError::NotImplemented(Feature::MediaBody)),
    };
    Ok(Request {
        id: Some(text(object.get("id"))?),
        method,
        url: text(object.get("url"))?,
        headers,
        body,
        depends_on,
    })
}

/// A JSON batch body: `{"requests": [...]}`, each member with an `id`, a `method`, a `url` and
/// optional `headers`, `body` and `dependsOn` (earlier ids only). A JSON body is kept as
/// compact JSON, a `text/*` body as its string.
///
/// ```
/// use odata_batch::{parse_json, Limits, Method};
/// let body = br#"{"requests":[
///   {"id":"1","method":"post","url":"Buildings","headers":{"content-type":"application/json"},"body":{"id":"b1"}},
///   {"id":"2","method":"get","url":"Buildings('b1')","dependsOn":["1"]}]}"#;
/// let batch = parse_json(body, &Limits::strict())?;
/// assert_eq!(batch.requests[0].body, br#"{"id":"b1"}"#);
/// assert_eq!(batch.requests[1].depends_on, [0]);
/// # Ok::<(), odata_batch::BatchError>(())
/// ```
///
/// # Errors
///
/// [`BatchError`], by the first problem found.
pub fn parse_json(body: &[u8], limits: &Limits) -> Result<Batch, BatchError> {
    if body.len() > limits.max_body_bytes {
        return Err(BatchError::limit(
            BatchLimit::BodyBytes,
            limits.max_body_bytes,
        ));
    }
    // The envelope is three levels: the batch object, `requests`, a member.
    if json_depth(body) > limits.max_json_depth.saturating_add(3) {
        return Err(BatchError::limit(
            BatchLimit::JsonDepth,
            limits.max_json_depth,
        ));
    }
    let value: Value = serde_json::from_slice(body).map_err(|e| BatchError::Json {
        line: e.line(),
        column: e.column(),
    })?;
    let Value::Object(top) = value else {
        return Err(BatchError::Malformed);
    };
    if top.keys().any(|k| k != "requests" && !k.starts_with('@')) {
        return Err(BatchError::Malformed);
    }
    let Some(Value::Array(members)) = top.get("requests") else {
        return Err(BatchError::Malformed);
    };
    if members.len() > limits.max_requests {
        return Err(BatchError::limit(BatchLimit::Requests, limits.max_requests));
    }
    let mut batch = Batch::default();
    for item in members {
        let Value::Object(object) = item else {
            return Err(BatchError::Malformed);
        };
        let request = member(object, &batch.requests, limits)?;
        check_member(&request, &batch.requests, limits)?;
        batch.requests.push(request);
    }
    Ok(batch)
}
