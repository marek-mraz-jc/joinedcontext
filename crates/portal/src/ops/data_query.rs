//! `jc_data_query`, the data read of the connector tools (API/14 §4.2, AG-146): one Endpoint's
//! entities, current or history, read through the gateway as the caller. The Portal never reads
//! with a token of its own here: it exchanges the token the caller presented for one the gateway
//! takes for that Endpoint alone, so the Endpoint's Policy decides on the person's live grants.
//! The answer stays entity-shaped, so the connector door's personal-data guard applies to it.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::bounds;
use super::{parse_input, Caller, OpError, Operation, OperationAnnotations};
use crate::change::Lane;
use crate::error::ApiError;
use crate::state::AppState;

/// The operation's registry name, the connector tools' `query_data` (API/14 §4.2).
pub const NAME: &str = "jc_data_query";
/// The most entities one page answers (NGSI-LD `limit`, the broker's own ceiling).
const MAX_LIMIT: u32 = 1000;
/// The page when the caller names none.
const DEFAULT_LIMIT: u32 = 20;
/// How long the gateway gets to answer one read; its own cut is 10 s (T-5349).
pub(super) const GATEWAY_TIMEOUT: Duration = Duration::from_secs(12);
/// The most bytes of one page read back: the gateway's own body ceiling (8 MiB).
const MAX_BODY: usize = 8 * 1024 * 1024;

/// The query string of one gateway read, as key and value pairs.
pub type Query = Vec<(String, String)>;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataQueryInput {
    pub endpoint: String,
    #[serde(default, rename = "type")]
    pub entity_type: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub attrs: Vec<String>,
    #[serde(default)]
    pub georel: Option<String>,
    #[serde(default)]
    pub geometry: Option<String>,
    #[serde(default)]
    pub coordinates: Option<Value>,
    #[serde(default)]
    pub order_by: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub temporal: Option<Temporal>,
    #[serde(default)]
    pub count: bool,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Temporal {
    pub timerel: String,
    pub time_at: String,
    #[serde(default)]
    pub end_time_at: Option<String>,
    #[serde(default)]
    pub last_n: Option<u32>,
    #[serde(default)]
    pub aggr_methods: Vec<String>,
    #[serde(default)]
    pub aggr_period_duration: Option<String>,
}

fn input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "endpoint": text("Endpoint name or public slug, e.g. city-counters", NAME),
            "type": text("The entity type, e.g. BikeCounter", TERM),
            "id": text("One entity's id, e.g. urn:ngsi-ld:BikeCounter:c1", ID),
            "q": text("An NGSI-LD q, e.g. count>40;name~=\"Most\"", QUERY),
            "attrs": texts("The attributes answered, e.g. [\"count\"]", TERM),
            "georel": text("near;maxDistance==500, within, intersects, contains, disjoint, equals or overlaps", TERM),
            "geometry": { "type": "string", "description": "The GeoJSON type of coordinates", "enum": ["Point", "LineString", "Polygon", "MultiPoint", "MultiLineString", "MultiPolygon"] },
            "coordinates": { "type": "array", "description": "GeoJSON coordinates, longitude first, e.g. [19.146, 48.736]" },
            "orderBy": text("attr or attr;desc, e.g. modifiedAt;desc", TERM),
            "format": { "type": "string", "description": "keyValues answers plain values", "enum": ["normalized", "concise", "keyValues"] },
            "temporal": {
                "type": "object",
                "description": "History instead of current values",
                "properties": {
                    "timerel": { "type": "string", "description": "Against timeAt (and endTimeAt)", "enum": ["before", "after", "between"] },
                    "timeAt": text("ISO 8601, e.g. 2026-10-27T00:00:00Z", TERM),
                    "endTimeAt": text("ISO 8601, the end of a between", TERM),
                    "lastN": { "type": "integer", "description": "The newest N per attribute", "minimum": 1, "maximum": MAX_LIMIT },
                    "aggrMethods": {
                        "type": "array",
                        "description": "Aggregates; per aggrPeriodDuration when given",
                        "items": { "type": "string", "enum": ["avg", "min", "max", "sum", "totalCount", "distinctCount", "stddev", "sumsq"] },
                        "maxItems": 8
                    },
                    "aggrPeriodDuration": text("ISO 8601 duration, e.g. PT1H", TERM)
                },
                "required": ["timerel", "timeAt"],
                "additionalProperties": false
            },
            "count": { "type": "boolean", "description": "Answer the total of matching entities too" },
            "limit": { "type": "integer", "minimum": 0, "maximum": MAX_LIMIT, "description": "Entities per page; 0 with count answers the total alone" },
            "cursor": text("The nextCursor of the previous page", TERM)
        },
        "required": ["endpoint"],
        "additionalProperties": false
    })
}

fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "entities": { "type": "array", "items": { "type": "object" } },
            "total": { "type": "integer" },
            "nextCursor": { "type": "string" }
        },
        "required": ["entities"]
    })
}

fn invalid(path: &str, message: impl Into<String>) -> OpError {
    OpError::InvalidInput {
        path: path.into(),
        message: message.into(),
    }
}

/// The page offset a cursor carries: the decimal offset this tool answered as `nextCursor`.
fn offset_of(cursor: Option<&str>) -> Result<u64, OpError> {
    match cursor {
        None => Ok(0),
        Some(c) => c.parse::<u64>().map_err(|_| {
            invalid(
                "/cursor",
                "not a cursor this tool answered; send nextCursor as it came",
            )
        }),
    }
}

/// Adds one query pair.
fn put(query: &mut Query, key: &str, value: &str) {
    query.push((key.to_owned(), value.to_owned()));
}

/// The gateway path under the Endpoint and its query, from a checked input: what a person would
/// send to the Endpoint's NGSI-LD API themselves.
pub fn request_of(input: &DataQueryInput) -> Result<(&'static str, Query), OpError> {
    let mut query = Query::new();
    for (key, value) in [
        ("type", &input.entity_type),
        ("id", &input.id),
        ("q", &input.q),
        ("orderBy", &input.order_by),
    ] {
        if let Some(value) = value.as_deref().filter(|v| !v.is_empty()) {
            put(&mut query, key, value);
        }
    }
    if !input.attrs.is_empty() {
        put(&mut query, "attrs", &input.attrs.join(","));
    }
    geo_of(input, &mut query)?;
    page_of(input, &mut query)?;
    let Some(temporal) = &input.temporal else {
        if let Some(format) = &input.format {
            put(&mut query, "format", format);
        }
        return Ok(("ngsi-ld/v1/entities", query));
    };
    temporal_of(temporal, input.format.as_deref(), &mut query)?;
    Ok(("ngsi-ld/v1/temporal/entities", query))
}

/// The geo-query pairs: georel, geometry and coordinates together or none of them.
fn geo_of(input: &DataQueryInput, query: &mut Query) -> Result<(), OpError> {
    match (&input.georel, &input.geometry, &input.coordinates) {
        (None, None, None) => Ok(()),
        (Some(georel), Some(geometry), Some(coordinates)) if coordinates.is_array() => {
            put(query, "georel", georel);
            put(query, "geometry", geometry);
            put(query, "coordinates", &coordinates.to_string());
            Ok(())
        }
        _ => Err(invalid(
            "/georel",
            "a geo-query takes georel, geometry and coordinates together, e.g. near;maxDistance==500, Point, [19.146, 48.736]",
        )),
    }
}

/// The paging pairs: limit, the cursor's offset and count.
fn page_of(input: &DataQueryInput, query: &mut Query) -> Result<(), OpError> {
    let limit = input.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 && !input.count {
        return Err(invalid(
            "/limit",
            "limit 0 answers nothing unless count is true",
        ));
    }
    put(query, "limit", &limit.to_string());
    let offset = offset_of(input.cursor.as_deref())?;
    if offset > 0 {
        put(query, "offset", &offset.to_string());
    }
    if input.count {
        put(query, "count", "true");
    }
    Ok(())
}

/// The history pairs: the time window, lastN and either the format asked or an aggregation.
fn temporal_of(
    temporal: &Temporal,
    format: Option<&str>,
    query: &mut Query,
) -> Result<(), OpError> {
    if chrono::DateTime::parse_from_rfc3339(&temporal.time_at).is_err() {
        return Err(invalid(
            "/temporal/timeAt",
            "an ISO 8601 time, e.g. 2026-10-27T00:00:00Z",
        ));
    }
    match (temporal.timerel.as_str(), &temporal.end_time_at) {
        ("between", None) => {
            return Err(invalid("/temporal/endTimeAt", "between needs endTimeAt"));
        }
        (_, Some(end)) if chrono::DateTime::parse_from_rfc3339(end).is_err() => {
            return Err(invalid(
                "/temporal/endTimeAt",
                "an ISO 8601 time, e.g. 2026-10-28T00:00:00Z",
            ));
        }
        _ => {}
    }
    put(query, "timerel", &temporal.timerel);
    put(query, "timeAt", &temporal.time_at);
    if let Some(end) = &temporal.end_time_at {
        put(query, "endTimeAt", end);
    }
    if let Some(n) = temporal.last_n {
        put(query, "lastN", &n.to_string());
    }
    match (
        temporal.aggr_methods.is_empty(),
        &temporal.aggr_period_duration,
    ) {
        (true, None) => {
            if let Some(format) = format {
                put(query, "format", format);
            }
        }
        (false, period) => {
            put(query, "format", "aggregatedValues");
            put(query, "aggrMethods", &temporal.aggr_methods.join(","));
            if let Some(period) = period {
                put(query, "aggrPeriodDuration", period);
            }
        }
        (true, Some(_)) => {
            return Err(invalid(
                "/temporal/aggrMethods",
                "aggrPeriodDuration needs aggrMethods, e.g. [\"avg\"]",
            ));
        }
    }
    Ok(())
}

/// The gateway's own words for a refusal, when its body has them.
pub(super) fn detail_of(body: &Value) -> String {
    body.get("detail")
        .or_else(|| body.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("no reason given")
        .chars()
        .take(500)
        .collect()
}

/// The Endpoint a read goes to: one of the project, read as the caller, or a public one of any
/// project named by its slug, read anonymously so a connector never reads more there than an
/// anonymous caller does (T-6272), or one of the project whose manifest the caller may not read,
/// read as the caller so the gateway decides whether a Policy shares it with them (ADR-N-067).
#[derive(Debug, PartialEq)]
pub(super) enum Target {
    Own(String),
    Public(String),
    Shared(String),
}

/// The Endpoint `name` names: the project's Endpoint of that name when the caller may read its
/// manifest, else the public Endpoint with that slug; any other case reads as a missing Endpoint,
/// so a refusal tells nothing about Endpoints the caller cannot see.
pub(super) fn readable_slug(
    caller: &Caller,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<Target, OpError> {
    let missing = || {
        OpError::Api(ApiError::NotFound(format!(
            "no Endpoint {name} in project {project}, and no public Endpoint has slug {name}"
        )))
    };
    let mut shared = None;
    let own = match state.mirror.get(project, "Endpoint", name) {
        Some(envelope) => {
            let manifest = serde_json::to_value(&envelope)?;
            // A space-living kind: the grant of the Endpoint's own space decides (T-5112).
            let readable = crate::permissions::for_request(state, &caller.identity, project)
                .may_read_manifest("Endpoint", &manifest);
            if !readable && envelope.spec["audience"].as_str() != Some("public") {
                shared = envelope.spec["slug"].as_str().map(str::to_owned);
            }
            readable.then_some(envelope)
        }
        None => None,
    };
    if let Some(envelope) = own {
        let slug = envelope.spec["slug"].as_str().ok_or_else(|| {
            ApiError::Unavailable(format!(
                "Endpoint {name} has no slug yet; read it again once it is served"
            ))
        })?;
        return Ok(Target::Own(slug.to_owned()));
    }
    if let Some(target) =
        crate::api::catalogue::public_endpoint(state, name).map(|_| Target::Public(name.to_owned()))
    {
        return Ok(target);
    }
    shared.map(Target::Shared).ok_or_else(missing)
}

/// The one answer for an Endpoint the gateway does not let the caller read as shared with them:
/// the same as for one that is not there.
pub(super) fn unshared(project: &str, name: &str) -> OpError {
    OpError::Api(ApiError::NotFound(format!(
        "no Endpoint {name} in project {project}, and no public Endpoint has slug {name}"
    )))
}

/// A 404 on the caller's own Endpoint the gateway answers because it does not admit them: a
/// refusal that names the fix. Any other answer goes on to speak for itself.
pub(super) fn not_admitted(
    status: u16,
    caller: &Caller,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<(), OpError> {
    let endpoint = state
        .mirror
        .get(project, "Endpoint", name)
        .filter(|_| status == 404);
    let why = endpoint.and_then(|endpoint| {
        super::endpoint_reach::not_admitted(state, &caller.identity, project, &endpoint.spec)
    });
    match why {
        Some(why) => Err(OpError::Api(ApiError::Denied(format!(
            "Endpoint {name}: {why}"
        )))),
        None => Ok(()),
    }
}

/// The gateway's status for an NGSI-LD read, as the error the caller can act on; `Ok` on 200.
fn refusal_of(
    status: u16,
    history: bool,
    name: &str,
    slug: &str,
    body: &Value,
) -> Result<(), OpError> {
    match status {
        200 => Ok(()),
        400 => Err(OpError::Api(ApiError::BadRequest(format!("the Endpoint refused the query: {}", detail_of(body))))),
        401 | 403 => Err(OpError::Api(ApiError::Denied(format!(
            "Endpoint {name} does not let you read this: {}",
            detail_of(body)
        )))),
        404 if history => Err(OpError::Api(ApiError::NotFound(format!(
            "Endpoint {name} serves no history: its view lists no temporal read; ask without temporal"
        )))),
        // A 404 with a detail (an entity id the view lacks) names what is missing; one
        // without a detail is a gateway that serves no such Endpoint (T-6042).
        404 if body.get("detail").is_some() => Err(OpError::Api(ApiError::NotFound(format!(
            "Endpoint {name} found nothing: {}",
            detail_of(body)
        )))),
        404 => Err(OpError::Api(ApiError::NotFound(format!("the gateway does not serve Endpoint {name} yet")))),
        _ => {
            tracing::warn!(status, endpoint = slug, "the gateway failed a data read");
            Err(OpError::Api(ApiError::Unavailable(format!(
                "the gateway answered {status} for Endpoint {name}; try again shortly"
            ))))
        }
    }
}

/// The tool's answer from the gateway's body: the entities, the total when counted and the
/// next cursor while a full page may have more behind it.
fn answer_of(input: &DataQueryInput, body: Value, total: Option<u64>) -> Result<Value, OpError> {
    let entities = match body {
        Value::Array(entities) => entities,
        Value::Object(_) => vec![body],
        _ => {
            return Err(OpError::Api(ApiError::Unavailable(
                "the gateway answered something that is not entities".into(),
            )))
        }
    };
    let limit = u64::from(input.limit.unwrap_or(DEFAULT_LIMIT));
    let next = offset_of(input.cursor.as_deref())?.saturating_add(entities.len() as u64);
    let more =
        limit > 0 && entities.len() as u64 == limit && total.is_none_or(|total| next < total);
    let mut out = json!({ "entities": entities });
    if let Some(total) = total {
        out["total"] = json!(total);
    }
    if more {
        out["nextCursor"] = json!(next.to_string());
    }
    Ok(out)
}

pub(super) async fn run(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: DataQueryInput,
) -> Result<Value, OpError> {
    let (path, query) = request_of(&input)?;
    let name = &input.endpoint;
    let target = readable_slug(caller, state, project, name)?;
    let shared = matches!(target, Target::Shared(_));
    let own = matches!(target, Target::Own(_));
    // A shared Endpoint answers every failure before the gateway's 200 (no bearer, a realm
    // refusal, a gateway status or outage) as a missing one: any other text would tell a caller
    // without a right that it exists, and some would name its slug.
    let hide = |err: OpError| {
        if shared {
            tracing::info!(error = %err, endpoint = name.as_str(), "a shared Endpoint read failed; answered as missing");
            unshared(project, name)
        } else {
            err
        }
    };
    let base = state
        .config
        .gateway_url
        .as_deref()
        .ok_or_else(|| {
            OpError::from(ApiError::Unavailable(
                "this Portal has no gateway address (JC_PORTAL_GATEWAY_URL)".into(),
            ))
        })
        .map_err(hide)?;
    let (slug, token) = match target {
        Target::Own(slug) | Target::Shared(slug) => {
            let token = endpoint_token(caller, state, &slug).await.map_err(hide)?;
            (slug, Some(token))
        }
        Target::Public(slug) => (slug, None),
    };
    let answer = gateway_read(base, &slug, path, &query, token, GATEWAY_TIMEOUT)
        .await
        .map_err(|err| {
            tracing::warn!(error = %err.without_url(), endpoint = slug, "the gateway did not answer a data read");
            OpError::from(ApiError::Unavailable("the gateway did not answer; try again shortly".into()))
        })
        .map_err(hide)?;
    let status = answer.status().as_u16();
    let total = answer
        .headers()
        .get("ngsild-results-count")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if shared && status != 200 {
        tracing::info!(
            status,
            endpoint = slug,
            "the gateway refused a shared Endpoint read; answered as missing"
        );
        return Err(unshared(project, name));
    }
    let bytes = capped_page(answer).await?;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    // The caller's groups mean nothing on a public read, and a project Endpoint named like its
    // slug may be one they cannot see (T-6451 review).
    if own {
        not_admitted(status, caller, state, project, name)?;
    }
    refusal_of(status, input.temporal.is_some(), name, &slug, &body)?;
    answer_of(&input, body, total)
}

/// A read of `path` under Endpoint `slug` at the gateway: anonymous without a bearer, so the
/// gateway decides on the token's grants or the Endpoint's public audience.
pub(crate) async fn gateway_read<Q: serde::Serialize + ?Sized>(
    base: &str,
    slug: &str,
    path: &str,
    query: &Q,
    bearer: Option<String>,
    timeout: std::time::Duration,
) -> reqwest::Result<reqwest::Response> {
    let request = crate::api::pipelines::http()
        .get(format!(
            "{}/api/endpoint/{slug}/{path}",
            base.trim_end_matches('/')
        ))
        .query(query)
        .header("accept", "application/json")
        .timeout(timeout);
    match bearer {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
    .send()
    .await
}

/// The person's token for Endpoint `slug`: their MCP client's bearer exchanged at the realm.
async fn endpoint_token(caller: &Caller, state: &AppState, slug: &str) -> Result<String, OpError> {
    let bearer = caller.bearer.as_ref().ok_or_else(|| {
        ApiError::Denied(
            "a data read goes as you with the token your MCP client presented: call it \
             through /api/v1/mcp or a connector"
                .into(),
        )
    })?;
    let oidc = state.oidc.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no Keycloak client to read data with".into())
    })?;
    Ok(oidc.exchange_for_endpoint(&bearer.0, slug).await?)
}

/// The gateway's page, read to [`MAX_BODY`]: a larger one is refused at the first byte past the
/// cap, never held whole first (F-R2POR4-009).
pub(super) async fn capped_page(answer: reqwest::Response) -> Result<Vec<u8>, OpError> {
    jc_core::outbound::read_capped(answer, MAX_BODY)
        .await
        .map_err(|err| match err {
            jc_core::outbound::BodyError::TooLarge(_) => invalid(
                "/limit",
                "the page is larger than 8 MiB: ask for fewer entities or attributes",
            ),
            jc_core::outbound::BodyError::Transport(_) => OpError::from(ApiError::Unavailable(
                "the gateway's answer broke off; try again".into(),
            )),
        })
}

pub fn operations() -> Vec<Operation> {
    vec![Operation {
        name: NAME,
        title: "Query Data",
        description: "Reads one Endpoint's entities, current or history (temporal), by type, id, q, attrs, geo-query, count and paging",
        input: input_schema,
        output: output_schema,
        annotations: OperationAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
        },
        kind: "Endpoint",
        verb: None,
        lane: Lane::Green,
        validate: |val| parse_input::<DataQueryInput>(val.clone()).and_then(|input| request_of(&input).map(|_| ())),
        run: |caller, state, project, val| {
            Box::pin(async move { run(caller, state, project, parse_input(val)?).await })
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-R2POR4-009: a page past 8 MiB is the `/limit` refusal, read only to the cap; one at
    /// the cap is read whole.
    #[tokio::test]
    async fn a_page_past_the_cap_is_refused_at_limit() {
        let page =
            |size: usize| reqwest::Response::from(axum::http::Response::new(vec![b' '; size]));
        let refused = capped_page(page(MAX_BODY + 1))
            .await
            .expect_err("past the cap");
        assert!(
            matches!(&refused, OpError::InvalidInput { path, .. } if path == "/limit"),
            "{refused}"
        );
        assert_eq!(
            capped_page(page(MAX_BODY)).await.expect("at the cap").len(),
            MAX_BODY
        );
    }

    fn input(args: Value) -> DataQueryInput {
        parse_input(args).expect("input parses")
    }

    fn query(args: Value) -> (&'static str, Query) {
        request_of(&input(args)).expect("a query")
    }

    fn param<'a>(q: &'a [(String, String)], key: &str) -> Option<&'a str> {
        q.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn a_broker_404_names_the_missing_entity_and_a_bare_404_the_endpoint() {
        let broker = json!({ "type": "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound",
                             "detail": "urn:ngsi-ld:BikeCounter:x" });
        let refused = |body: &Value| match refusal_of(404, false, "counters", "s", body) {
            Err(OpError::Api(ApiError::NotFound(text))) => text,
            other => panic!("not a 404 refusal: {other:?}"),
        };
        assert_eq!(
            refused(&broker),
            "Endpoint counters found nothing: urn:ngsi-ld:BikeCounter:x"
        );
        assert_eq!(
            refused(&Value::Null),
            "the gateway does not serve Endpoint counters yet"
        );
    }

    #[test]
    fn a_current_read_carries_each_filter_and_the_format_asked() {
        for format in ["normalized", "concise", "keyValues"] {
            let (path, q) = query(json!({
                "endpoint": "doprava-counters", "type": "BikeCounter", "q": "count>40",
                "attrs": ["count", "name"], "orderBy": "modifiedAt;desc", "format": format, "limit": 10
            }));
            assert_eq!(path, "ngsi-ld/v1/entities");
            assert_eq!(param(&q, "type"), Some("BikeCounter"));
            assert_eq!(param(&q, "q"), Some("count>40"));
            assert_eq!(param(&q, "attrs"), Some("count,name"));
            assert_eq!(param(&q, "orderBy"), Some("modifiedAt;desc"));
            assert_eq!(param(&q, "format"), Some(format));
            assert_eq!(param(&q, "limit"), Some("10"));
            assert_eq!(param(&q, "offset"), None);
        }
    }

    #[test]
    fn a_geo_query_goes_whole_or_is_refused() {
        let (_, q) = query(json!({
            "endpoint": "e", "type": "ParkingSpot", "georel": "near;maxDistance==500",
            "geometry": "Point", "coordinates": [19.146, 48.736]
        }));
        assert_eq!(param(&q, "coordinates"), Some("[19.146,48.736]"));
        assert_eq!(param(&q, "georel"), Some("near;maxDistance==500"));
        let half = input(json!({ "endpoint": "e", "georel": "near;maxDistance==500" }));
        assert!(
            matches!(request_of(&half), Err(OpError::InvalidInput { path, .. }) if path == "/georel")
        );
    }

    #[test]
    fn a_count_alone_takes_limit_zero_and_a_cursor_is_an_offset() {
        let (_, q) = query(json!({ "endpoint": "e", "type": "T", "count": true, "limit": 0 }));
        assert_eq!(param(&q, "count"), Some("true"));
        assert_eq!(param(&q, "limit"), Some("0"));
        let nothing = input(json!({ "endpoint": "e", "type": "T", "limit": 0 }));
        assert!(
            matches!(request_of(&nothing), Err(OpError::InvalidInput { path, .. }) if path == "/limit")
        );
        let (_, q) = query(json!({ "endpoint": "e", "type": "T", "cursor": "40" }));
        assert_eq!(param(&q, "offset"), Some("40"));
        let forged = input(json!({ "endpoint": "e", "type": "T", "cursor": "40;drop" }));
        assert!(
            matches!(request_of(&forged), Err(OpError::InvalidInput { path, .. }) if path == "/cursor")
        );
    }

    #[test]
    fn a_history_read_aggregates_and_checks_its_times() {
        let (path, q) = query(json!({
            "endpoint": "e", "id": "urn:ngsi-ld:BikeCounter:c1", "attrs": ["count"],
            "temporal": { "timerel": "between", "timeAt": "2026-10-27T00:00:00Z",
                          "endTimeAt": "2026-10-28T00:00:00Z", "aggrMethods": ["avg"], "aggrPeriodDuration": "PT1H" }
        }));
        assert_eq!(path, "ngsi-ld/v1/temporal/entities");
        assert_eq!(param(&q, "format"), Some("aggregatedValues"));
        assert_eq!(param(&q, "aggrMethods"), Some("avg"));
        assert_eq!(param(&q, "aggrPeriodDuration"), Some("PT1H"));
        assert_eq!(param(&q, "endTimeAt"), Some("2026-10-28T00:00:00Z"));
        for (temporal, at) in [
            (
                json!({ "timerel": "between", "timeAt": "2026-10-27T00:00:00Z" }),
                "/temporal/endTimeAt",
            ),
            (
                json!({ "timerel": "after", "timeAt": "yesterday" }),
                "/temporal/timeAt",
            ),
            (
                json!({ "timerel": "after", "timeAt": "2026-10-27T00:00:00Z", "aggrPeriodDuration": "PT1H" }),
                "/temporal/aggrMethods",
            ),
        ] {
            let bad = input(json!({ "endpoint": "e", "type": "T", "temporal": temporal }));
            assert!(
                matches!(request_of(&bad), Err(OpError::InvalidInput { path, .. }) if path == at),
                "{at}"
            );
        }
    }

    #[test]
    fn the_schema_refuses_an_unknown_member_and_a_page_past_the_ceiling() {
        let schema = jsonschema::validator_for(&input_schema()).expect("schema compiles");
        assert!(schema.is_valid(&json!({ "endpoint": "e", "type": "T" })));
        assert!(!schema.is_valid(&json!({ "endpoint": "e", "sql": "select 1" })));
        assert!(!schema.is_valid(&json!({ "endpoint": "e", "limit": 1001 })));
        assert!(!schema.is_valid(&json!({ "type": "T" })));
    }

    const PUBLIC: &str = "stopshelsinkizzzzzzzzzzzzzzz";
    const PRIVATE: &str = "privatehelsinkizzzzzzzzzzzz";

    /// A Portal whose mirror holds Helsinki's public stops Endpoint and a project-only one, and a
    /// person of another project with no grant in Helsinki (T-6272).
    fn helsinki(gateway: Option<String>) -> (AppState, Caller) {
        let mut config = crate::config::Config::for_tests();
        config.gateway_url = gateway;
        let state = AppState::new(config, None);
        for (name, slug, audience) in [("stops", PUBLIC, "public"), ("staff", PRIVATE, "project")] {
            state.mirror.upsert(crate::resource::ResourceEnvelope {
                api_version: crate::resource::API_VERSION.to_owned(),
                kind: "Endpoint".to_owned(),
                metadata: crate::resource::ObjectMeta::new(name, "helsinki"),
                spec: json!({ "slug": slug, "audience": audience }),
                status: None,
            });
        }
        let caller = Caller {
            identity: crate::auth::session::Identity {
                client: None,
                subject: "sub-full".into(),
                username: "t6025-full".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: false,
            },
            via: super::super::Via::Mcp,
            access: None,
            bearer: Some(super::super::Bearer("person-token".into())),
        };
        (state, caller)
    }

    #[test]
    fn a_public_slug_of_another_project_is_readable_and_nothing_else_is() {
        let (state, caller) = helsinki(None);
        assert_eq!(
            readable_slug(&caller, &state, "t6025-full", PUBLIC).expect("public"),
            Target::Public(PUBLIC.into())
        );
        // A project-only Endpoint of another project, by slug or by name, and a public one by
        // its name: each reads as missing, the same words as an unknown name.
        for name in [PRIVATE, "staff", "stops", "helsinki-transport"] {
            match readable_slug(&caller, &state, "t6025-full", name) {
                Err(OpError::Api(ApiError::NotFound(text))) => {
                    assert!(
                        text.starts_with(&format!("no Endpoint {name} in project t6025-full")),
                        "{text}"
                    );
                }
                other => panic!("{name} is not missing: {other:?}"),
            }
        }
    }

    /// The answer is the anonymous gateway read itself: no token goes with it, the person's
    /// own least of all.
    #[tokio::test]
    async fn a_public_endpoint_is_read_anonymously_with_the_gateway_answer() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let gateway = MockServer::start().await;
        let stops = json!([{ "id": "urn:ngsi-ld:GtfsStop:1", "type": "GtfsStop" }]);
        Mock::given(method("GET"))
            .and(path(format!("/api/endpoint/{PUBLIC}/ngsi-ld/v1/entities")))
            .and(query_param("type", "GtfsStop"))
            .and(query_param("georel", "within"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ngsild-results-count", "14")
                    .set_body_json(&stops),
            )
            .expect(1)
            .mount(&gateway)
            .await;
        let (state, caller) = helsinki(Some(gateway.uri()));
        let polygon = json!([[[24.9, 60.1], [25.0, 60.1], [25.0, 60.2], [24.9, 60.1]]]);
        let answer = run(
            &caller,
            &state,
            "t6025-full",
            input(
                json!({ "endpoint": PUBLIC, "type": "GtfsStop", "georel": "within",
                          "geometry": "Polygon", "coordinates": polygon, "count": true }),
            ),
        )
        .await
        .expect("an anonymous read");
        assert_eq!(answer, json!({ "entities": stops, "total": 14 }));
        let sent = gateway.received_requests().await.expect("recorded");
        assert!(
            sent.iter()
                .all(|r| !r.headers.contains_key("authorization")),
            "a public read carries no token"
        );
    }

    /// T-6451 review: a project Endpoint named like a public slug, which the caller cannot read,
    /// stays out of a public read's refusal: their groups mean nothing there.
    #[tokio::test]
    async fn a_public_read_names_no_hidden_endpoint_of_the_same_name() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let gateway = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&gateway)
            .await;
        let (state, caller) = helsinki(Some(gateway.uri()));
        state.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: crate::resource::ObjectMeta::new(PUBLIC, "t6025-full"),
            spec: json!({ "audience": "organization" }),
            status: None,
        });
        state.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "Organization".to_owned(),
            metadata: crate::resource::ObjectMeta::new("hel", crate::permissions::ORG_NAMESPACE),
            spec: json!({}),
            status: None,
        });
        let err = run(
            &caller,
            &state,
            "t6025-full",
            input(json!({ "endpoint": PUBLIC, "type": "GtfsStop" })),
        )
        .await
        .expect_err("the gateway answered 404");
        let text = err.to_string();
        assert!(!text.contains("audience"), "{text}");
    }

    /// Every way a read of a shared Endpoint can fail before the gateway's 200 answers in the
    /// words of a missing Endpoint, so a caller without a right learns neither that it exists
    /// nor its slug: no bearer, a realm refusing the exchange, a gateway 5xx (T-6413 review).
    #[tokio::test]
    async fn a_shared_endpoint_fails_in_the_words_of_a_missing_one() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let realm = crate::auth::oidc::tests::mock_realm().await;
        let exchange = |code: &str| {
            ResponseTemplate::new(400).set_body_json(json!({
                "error": code, "error_description": "scope not linked"
            }))
        };
        Mock::given(method("POST"))
            .and(path("/realms/bb/protocol/openid-connect/token"))
            .respond_with(exchange("invalid_scope"))
            .up_to_n_times(1)
            .mount(&realm)
            .await;
        Mock::given(method("POST"))
            .and(path("/realms/bb/protocol/openid-connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "endpoint-token", "token_type": "Bearer", "expires_in": 300
            })))
            .mount(&realm)
            .await;
        let gateway = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/endpoint/{PRIVATE}/ngsi-ld/v1/entities")))
            .respond_with(ResponseTemplate::new(503).set_body_json(json!({ "detail": PRIVATE })))
            .mount(&gateway)
            .await;
        let (mut state, mut caller) = helsinki(Some(gateway.uri()));
        let client = crate::auth::oidc::OidcClient::discover(
            &crate::auth::oidc::tests::config_for(&realm),
            "https://portal.test/cb",
        )
        .await
        .expect("discovery");
        state.oidc = Some(std::sync::Arc::new(client));
        let read = || input(json!({ "endpoint": "staff", "type": "Person" }));
        let missing = run(
            &caller,
            &state,
            "helsinki",
            input(json!({ "endpoint": "nobody", "type": "Person" })),
        )
        .await
        .expect_err("a missing Endpoint");
        let words = |err: OpError| match err {
            OpError::Api(ApiError::NotFound(text)) => text.replace("nobody", "staff"),
            other => panic!("not the missing answer: {other:?}"),
        };
        let missing = words(missing);
        // The realm refuses the first exchange (invalid_scope), then the gateway answers 503.
        for case in ["realm invalid_scope", "gateway 503"] {
            let err = run(&caller, &state, "helsinki", read())
                .await
                .expect_err(case);
            let text = words(err);
            assert_eq!(text, missing, "{case}");
            assert!(!text.contains(PRIVATE), "{case} names the slug: {text}");
        }
        caller.bearer = None;
        let err = run(&caller, &state, "helsinki", read())
            .await
            .expect_err("no bearer");
        assert_eq!(words(err), missing, "no bearer");
    }
}
