// SPDX-License-Identifier: EUPL-1.2
//! The request parsers the fuzz targets under `fuzz/` drive (T-3463). Built
//! only with the `fuzzing` feature, which no broker build turns on: each
//! entry point hands one input to the parser a handler would hand it to and
//! answers whether it was accepted. A panic is the finding; a refusal is the
//! parser doing its job.

use std::collections::HashMap;
use std::sync::OnceLock;

use axum::http::{header, HeaderMap, HeaderName, HeaderValue};
use serde_json::json;

fn ctx() -> &'static antares_jsonld::Context {
    static CTX: OnceLock<antares_jsonld::Context> = OnceLock::new();
    CTX.get_or_init(antares_jsonld::core_context)
}

/// One header carrying `value` (several when `value` holds newlines: a
/// request may repeat a header), or `None` when the bytes are no header value.
fn headers(name: HeaderName, value: &[u8]) -> Option<HeaderMap> {
    let mut map = HeaderMap::new();
    for line in value.split(|b| *b == b'\n') {
        map.append(name.clone(), HeaderValue::from_bytes(line).ok()?);
    }
    Some(map)
}

/// 4.21 `pick` / `omit` (Attribute Projection Language).
pub fn projection(text: &str) -> bool {
    crate::repr::parse_projection(text, ctx()).is_ok()
}

/// `format`, `options`, `lang`, `pick`, `omit` and the rest of a
/// representation's query parameters.
pub fn representation(params: &HashMap<String, String>) -> bool {
    crate::repr::parse_repr(params, ctx()).is_ok()
}

/// 4.5.23 `join`, `joinLevel`, `containedBy`.
pub fn join(params: &HashMap<String, String>) -> bool {
    crate::entities::parse_join(params).is_ok()
}

/// 4.11 `timerel`, `timeAt`, `endTimeAt`, `timeproperty`, and the temporal
/// representation parameters (`lastN`, `aggrMethods`, `aggrPeriodDuration` …).
pub fn temporal(params: &HashMap<String, String>) -> bool {
    let query = crate::temporalq::TemporalQ::from_params(params, false).is_ok();
    let representation = crate::temporal::parse_trepr(params, ctx()).is_ok();
    let period = params
        .get("aggrPeriodDuration")
        .is_some_and(|p| crate::temporal::parse_aggr_period(p).is_some());
    query && representation && period
}

/// 4.23 `orderBy`, applied to two entities so the parsed keys are used.
pub fn order_by(spec: &str, params: &HashMap<String, String>) -> bool {
    let mut docs = [
        json!({"id": "urn:ngsi-ld:T:1", "type": "T"}),
        json!({"id": "urn:ngsi-ld:T:2", "type": "T", "location": [{"value": {"type": "Point", "coordinates": [1.0, 2.0]}}]}),
    ];
    crate::paging::order_entities(&mut docs, spec, params, ctx()).is_ok()
}

/// 4.23.4 `collation`.
pub fn collation(params: &HashMap<String, String>) -> bool {
    crate::paging::check_collation(params).is_ok()
}

/// The `Link` header naming a request's @context (TS 104 176 6.2.4).
pub fn link_header(value: &[u8]) -> bool {
    headers(header::LINK, value).is_some_and(|h| crate::negotiate::link_context(&h).is_ok())
}

/// `NGSILD-Tenant`.
pub fn tenant_header(value: &[u8]) -> bool {
    headers(HeaderName::from_static("ngsild-tenant"), value)
        .is_some_and(|h| crate::negotiate::tenant_from(&h).is_ok())
}

/// `Accept`, with GeoJSON offered.
pub fn accept_header(value: &[u8]) -> bool {
    headers(header::ACCEPT, value).is_some_and(|h| crate::negotiate::parse_accept_geo(&h).is_ok())
}
