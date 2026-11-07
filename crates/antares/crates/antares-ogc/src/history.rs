// SPDX-License-Identifier: EUPL-1.2
//! The `{type}-history` collections (T-5100): beside each collection whose view offers the
//! Temporal Query, one Feature per Entity and stored `observedAt`, read through the view's
//! Query Temporal Evolution of Entities, so the view's Policy and its shown Attributes narrow
//! the history as they narrow the current state. A Feature's `properties` are the values valid
//! at its `validFrom` (each Attribute's latest instance at or before it, the location too), and
//! `validTo` is the Entity's next `observedAt`, null for the newest: the interval QGIS'
//! Temporal Controller and the `ArcGIS Pro` time slider animate.
//!
//! `datetime` keeps the Features valid in the window, `bbox` and `filter` are evaluated in
//! process by the residual over each Feature. ponytail: each Entity's newest
//! [`INSTANCES`] instances per Attribute are read (the broker's own ceiling), so a window older
//! than them answers nothing; a `timerel` pushdown when histories outgrow it.

use crate::items::{self, bad, feature, read_token, token, value_of, Page, Params, Refs};
use crate::model::{Collection, Prop, Time};
use crate::terms::Terms;
use crate::Problem;
use antares_ops::residual::{self, DateTime, Expr, Lit, Path, Program, Spatial, Truth};
use antares_ops::{Caller, OperationId, OpsRequest, Pushdown, Scanned, ViewOps};
use axum::http::{Method, StatusCode};
use serde_json::{json, Map, Value};
use std::fmt::Write as _;

/// What a history collection's id adds to its type's.
pub(crate) const SUFFIX: &str = "-history";
/// The IRIs of the three members a history Feature adds; no Entity has them.
const ENTITY_ID: &str = "urn:antares:ogc:history:entityId";
const VALID_FROM: &str = "urn:antares:ogc:history:validFrom";
const VALID_TO: &str = "urn:antares:ogc:history:validTo";
/// The instances of each Attribute one read asks for: the newest, as many as the broker's
/// default `ANTARES_TEMPORAL_INSTANCE_LIMIT` serves.
pub(crate) const INSTANCES: usize = 1_000;
/// The Entities one Temporal Query reads, and the reads one page makes at most; past them a
/// page ends short with its `next` link.
const BATCH: usize = 4;
const MAX_READS: usize = 25;
/// The start of every history read (`timerel=after`).
const EPOCH: &str = "1970-01-01T00:00:00Z";
/// The longest encoded `attrs` list the Temporal Query names, half the broker's 8 KiB URI cap
/// (antares-api `bounds::MAX_URI_BYTES`), so the rest of the query fits beside it.
const ATTRS_BUDGET: usize = 4 * 1024;
/// The details of the refusals a read of the history answers.
const REFUSED: &str = "the view refused the query";
const NOT_NGSI_LD: &str =
    "the query is not valid NGSI-LD; an ngsild. parameter takes NGSI-LD syntax";
const NO_LIST: &str = "the temporal answer is no list of Entities";
const NO_CLOCK: &str = "the clock reads no time";
/// The `ngsild.` options a history takes: the Temporal Query reads them as Query Entities does.
const OPTIONS: [&str; 2] = ["q", "scopeQ"];

/// The history collection of `c`: its properties, none sortable and none a time role of its
/// own, then `entityId`, `validFrom` and `validTo`, the latter two its primary interval.
pub(crate) fn of(c: &Collection) -> Collection {
    let time_role = |r: &Option<String>| r.as_deref().is_some_and(|r| r.starts_with("primary-in"));
    let own = c.properties.iter().cloned().map(|mut p| {
        p.sortable = false;
        if time_role(&p.role) {
            p.role = None;
        }
        p
    });
    let stamp = json!({"type": "string", "format": "date-time"});
    let added = [
        (
            "entityId",
            ENTITY_ID,
            json!({"type": "string", "format": "uri"}),
            None,
            "The Entity this state is of",
        ),
        (
            "validFrom",
            VALID_FROM,
            stamp.clone(),
            Some("primary-interval-start"),
            "When these values were observed (observedAt)",
        ),
        (
            "validTo",
            VALID_TO,
            json!({"type": ["string", "null"], "format": "date-time"}),
            Some("primary-interval-end"),
            "When the Entity's next values were observed; null for its newest",
        ),
    ]
    .into_iter()
    .map(|(name, iri, schema, role, description)| Prop {
        name: name.to_owned(),
        iri: iri.to_owned(),
        schema,
        role: role.map(str::to_owned),
        unit: None,
        description: Some(description.to_owned()),
        sortable: false,
        kind: "Property".to_owned(),
        reference: false,
        target: None,
    });
    Collection {
        id: format!("{}{SUFFIX}", c.id),
        description: Some(format!(
            "The stored history of {}: one Feature per Entity and observedAt, its values valid from validFrom until validTo",
            c.id
        )),
        type_iri: c.type_iri.clone(),
        geometry: c.geometry.clone(),
        time: Some(Time::Interval(
            Some(VALID_FROM.to_owned()),
            Some(VALID_TO.to_owned()),
        )),
        properties: own.chain(added).collect(),
        history: true,
        output: None,
    }
}

/// One page of a history's Features: Entities read [`BATCH`] at a time from the token's
/// Entity offset, its Features from the token's Feature offset, until the page holds `limit`.
/// ponytail: a page that resumes inside an Entity reads its batch again (bounded by `MAX_READS` x
/// `BATCH` x `INSTANCES`); a token carrying the resume `observedAt` would cut that for small limits.
pub(crate) async fn page(
    view: &ViewOps,
    caller: &Caller,
    c: &Collection,
    p: &Params,
    digest: u64,
    refs: Refs<'_>,
) -> Result<Page, Problem> {
    let terms = Terms::of(c);
    let program = program(c, &terms, p)?;
    let (mut entity, mut skip) = match &p.next {
        None => (0, 0),
        Some(t) => read_token(t, digest, usize::MAX)
            .ok_or_else(|| bad("the next token does not belong to this request"))?,
    };
    let limit = usize::try_from(p.limit).unwrap_or(usize::MAX);
    let window = window(p);
    let refs = Refs {
        alternate: false,
        ..refs
    };
    let mut out = Page {
        features: Vec::new(),
        matched: None,
        next: None,
    };
    for _ in 0..MAX_READS {
        let batch = read(view, caller, c, &terms, p, None, entity).await?;
        let places = places(view, caller, c, &terms, &batch).await;
        for (i, e) in batch.iter().enumerate() {
            let place = place_of(&places, e);
            let states = states(c, &terms, e, place, window, program.as_ref())?;
            for (k, state) in states.iter().enumerate().skip(skip) {
                if out.features.len() >= limit {
                    out.next = Some(token(entity.saturating_add(i), k, digest));
                    return Ok(out);
                }
                out.features
                    .push(Value::Object(shaped(c, &terms, state, refs, p)));
            }
            skip = 0;
        }
        if batch.len() < BATCH {
            return Ok(out);
        }
        entity = entity.saturating_add(BATCH);
    }
    out.next = Some(token(entity, 0, digest));
    Ok(out)
}

/// One history Feature by its id, `{entity}@{observedAt}`.
pub(crate) async fn one(
    view: &ViewOps,
    caller: &Caller,
    c: &Collection,
    id: &str,
    p: &Params,
    refs: Refs<'_>,
) -> Result<Map<String, Value>, Problem> {
    let missing = Problem(StatusCode::NOT_FOUND, "no such Feature in this collection");
    let Some((urn, _)) = id.rsplit_once('@') else {
        return Err(missing);
    };
    options(p)?;
    let terms = Terms::of(c);
    let refs = Refs {
        alternate: false,
        ..refs
    };
    let batch = read(view, caller, c, &terms, p, Some(urn), 0).await?;
    let places = places(view, caller, c, &terms, &batch).await;
    for e in &batch {
        for state in states(c, &terms, e, place_of(&places, e), (None, None), None)? {
            if state.get("id").and_then(Value::as_str) == Some(id) {
                return Ok(shaped(c, &terms, &state, refs, p));
            }
        }
    }
    Err(missing)
}

/// The `datetime` window in microseconds, either end open.
type Window = (Option<i64>, Option<i64>);

/// `params` already read each end as a calendar instant.
fn window(p: &Params) -> Window {
    let micros = |at: &Option<String>| {
        at.as_deref()
            .and_then(DateTime::parse)
            .map(DateTime::unix_micros)
    };
    p.datetime
        .as_ref()
        .map_or((None, None), |(from, to)| (micros(from), micros(to)))
}

/// The residual of `bbox` and `filter`, none when the request names neither; a `ngsild.`
/// option a history does not take is refused.
fn program(c: &Collection, terms: &Terms, p: &Params) -> Result<Option<Program>, Problem> {
    options(p)?;
    let mut all = match &p.filter {
        Some(f) => crate::filter::residual(c, terms, f)?,
        None => Vec::new(),
    };
    if let (Some(bbox), Some(geometry)) = (p.bbox, c.geometry.as_deref()) {
        let (kind, coordinates) = crate::crs::bbox_geometry(bbox);
        all.push(Expr::Spatial(
            Spatial::Intersects,
            Box::new(Expr::Path(Path::value(terms.term(geometry)))),
            Box::new(Expr::Lit(Lit::Geometry(
                json!({ "type": kind, "coordinates": coordinates }),
            ))),
        ));
    }
    if all.is_empty() {
        return Ok(None);
    }
    let now = items::now().ok_or(Problem(StatusCode::INTERNAL_SERVER_ERROR, NO_CLOCK))?;
    let compiled = residual::compile(&Expr::And(all), now);
    compiled.map(Some).map_err(|e| items::refused(&e))
}

/// A `ngsild.` option other than `q` and `scopeQ` is refused, on a page and on one Feature.
fn options(p: &Params) -> Result<(), Problem> {
    if p.ngsild.iter().any(|(n, _)| !OPTIONS.contains(n)) {
        return Err(bad(
            "a history collection takes ngsild.q and ngsild.scopeQ of the ngsild. options",
        ));
    }
    Ok(())
}

/// One Query Temporal Evolution of Entities through the view: [`BATCH`] Entities of the type
/// from `offset`, or the one `urn`, with the newest [`INSTANCES`] of each shown Attribute.
async fn read(
    view: &ViewOps,
    caller: &Caller,
    c: &Collection,
    terms: &Terms,
    p: &Params,
    urn: Option<&str>,
    offset: usize,
) -> Result<Vec<Value>, Problem> {
    let attrs: Vec<&str> = c
        .geometry
        .iter()
        .chain(c.properties.iter().map(|p| &p.iri))
        .filter(|iri| !matches!(iri.as_str(), ENTITY_ID | VALID_FROM | VALID_TO))
        .map(|iri| terms.term(iri))
        .collect();
    let mut path = format!(
        "/ngsi-ld/v1/temporal/entities?type={}&timerel=after&timeAt={EPOCH}&lastN={INSTANCES}&limit={BATCH}&offset={offset}",
        items::encode(&c.type_iri),
    );
    // A list past the budget is left out: the view's rules still narrow the answer to its
    // shown Attributes, and `states` reads only the collection's own.
    let attrs = items::encode(&attrs.join(","));
    if attrs.len() <= ATTRS_BUDGET {
        let _ = write!(path, "&attrs={attrs}");
    }
    if let Some(urn) = urn {
        let _ = write!(path, "&id={}", items::encode(urn));
    }
    for (name, value) in &p.ngsild {
        let _ = write!(path, "&{name}={}", items::encode(value));
    }
    let req = OpsRequest::new(Method::GET, path);
    let req = terms.context().into_iter().fold(req, OpsRequest::context);
    let answer = view.run(OperationId::TemporalQuery, caller, req).await;
    let answer = answer.map_err(|r| Problem(r.status(), REFUSED))?;
    match answer.status() {
        // A Tenant that holds nothing yet answers 404 (5.5.10): an empty history.
        StatusCode::NOT_FOUND => return Ok(Vec::new()),
        StatusCode::BAD_REQUEST => return Err(bad(NOT_NGSI_LD)),
        s if !s.is_success() => return Err(Problem(s, REFUSED)),
        _ => {}
    }
    let bytes = view.read_answer(answer.into_body(), "ogc").await;
    let bytes = bytes.map_err(|r| Problem(r.status(), REFUSED))?;
    serde_json::from_slice(&bytes).map_err(|_| Problem(StatusCode::BAD_GATEWAY, NO_LIST))
}

/// The current geometry of each Entity of `batch` whose history holds no place, by id, read
/// through the view's Query Entities (a station's location is written once, without
/// `observedAt`); none when the view offers no Query Entities or refuses it.
async fn places(
    view: &ViewOps,
    caller: &Caller,
    c: &Collection,
    terms: &Terms,
    batch: &[Value],
) -> Map<String, Value> {
    let Some(geometry) = c.geometry.as_deref().map(|g| terms.term(g)) else {
        return Map::new();
    };
    let ids: Vec<&str> = batch
        .iter()
        .filter(|e| instances(e, geometry).is_empty())
        .filter_map(|e| e.get("id").and_then(Value::as_str))
        .collect();
    if ids.is_empty() || !view.allows(OperationId::QueryEntities) {
        return Map::new();
    }
    let pushdown = Pushdown {
        query: format!(
            "id={}&type={}&format=simplified&pick={}",
            items::encode(&ids.join(",")),
            items::encode(&c.type_iri),
            items::encode(&format!("id,{geometry}"))
        ),
        context: terms.context(),
        ..Pushdown::default()
    };
    let page = antares_ops::Page {
        start: 0,
        size: ids.len(),
        whole: false,
    };
    let Ok(Scanned::Page { entities, .. }) = view.scan(caller, &pushdown, None, page).await else {
        return Map::new();
    };
    entities
        .into_iter()
        .filter_map(|e| {
            let id = e.get("id")?.as_str()?.to_owned();
            Some((id, e.get(geometry)?.clone()))
        })
        .collect()
}

/// The place [`places`] read for `e`.
fn place_of<'a>(places: &'a Map<String, Value>, e: &Value) -> Option<&'a Value> {
    places.get(e.get("id").and_then(Value::as_str).unwrap_or_default())
}

/// An Attribute's instances: time in microseconds, time as written, value.
type Instances = Vec<(i64, String, Value)>;

/// The instances of one Attribute of a temporal Entity: the default instance's (no `datasetId`)
/// time and value, oldest first; an instance without `observedAt` has no place in a history.
fn instances(e: &Value, term: &str) -> Instances {
    let all = match e.get(term) {
        Some(Value::Array(list)) => list.iter().collect(),
        Some(one @ Value::Object(_)) => vec![one],
        _ => Vec::new(),
    };
    let mut out: Instances = all
        .into_iter()
        .filter(|i| i.get("datasetId").is_none())
        .filter_map(|i| {
            let at = i.get("observedAt")?.as_str()?;
            let micros = DateTime::parse(at)?.unix_micros();
            Some((micros, at.to_owned(), value_of(i)))
        })
        .collect();
    out.sort_by_key(|(t, _, _)| *t);
    out
}

/// The states of one temporal Entity in the simplified form [`feature`] reads, oldest first:
/// one per distinct `observedAt`, kept when valid in `window` and true under `program`.
fn states(
    c: &Collection,
    terms: &Terms,
    e: &Value,
    place: Option<&Value>,
    (from, to): Window,
    program: Option<&Program>,
) -> Result<Vec<Value>, Problem> {
    let urn = e.get("id").and_then(Value::as_str).unwrap_or_default();
    let kinds = c
        .geometry
        .iter()
        .map(|g| (g, "GeoProperty"))
        .chain(c.properties.iter().map(|p| {
            let kind = if p.reference {
                "Relationship"
            } else {
                "Property"
            };
            (&p.iri, kind)
        }))
        .filter(|(iri, _)| !matches!(iri.as_str(), ENTITY_ID | VALID_FROM | VALID_TO));
    let attrs: Vec<(&str, &str, Instances)> = kinds
        .map(|(iri, kind)| {
            let term = terms.term(iri);
            (term, kind, instances(e, term))
        })
        .collect();
    let mut times: Vec<(i64, &str)> = attrs
        .iter()
        .flat_map(|(_, _, list)| list.iter().map(|(t, text, _)| (*t, text.as_str())))
        .collect();
    times.sort_by_key(|(t, _)| *t);
    times.dedup_by_key(|(t, _)| *t);
    let mut out = Vec::new();
    for (n, (at, text)) in times.iter().enumerate() {
        let until = times.get(n.saturating_add(1));
        let valid = to.is_none_or(|to| *at <= to)
            && from.is_none_or(|from| until.is_none_or(|(next, _)| *next > from));
        if !valid {
            continue;
        }
        let mut simple = Map::new();
        let mut normalized = Map::new();
        simple.insert("id".into(), json!(format!("{urn}@{text}")));
        normalized.insert("id".into(), json!(format!("{urn}@{text}")));
        normalized.insert("type".into(), json!(c.type_iri));
        for (term, kind, list) in &attrs {
            let valid = list.iter().rev().find(|(t, _, _)| t <= at).map(|i| &i.2);
            // A place written without observedAt is the Entity's place at every time.
            let fallback = place.filter(|_| *kind == "GeoProperty");
            let Some(value) = valid.or(fallback) else {
                continue;
            };
            simple.insert((*term).to_owned(), value.clone());
            let member = if *kind == "Relationship" {
                "object"
            } else {
                "value"
            };
            normalized.insert((*term).to_owned(), json!({"type": kind, member: value}));
        }
        let to_text = until.map(|(_, t)| *t);
        let stamps = [
            (ENTITY_ID, Some(Value::from(urn))),
            (VALID_FROM, Some(Value::from(*text))),
            (VALID_TO, to_text.map(Value::from)),
        ];
        for (iri, value) in stamps {
            let term = terms.term(iri).to_owned();
            simple.insert(term.clone(), value.clone().unwrap_or(Value::Null));
            if let Some(v) = value {
                let v = if iri == ENTITY_ID {
                    v
                } else {
                    json!({"@type": "DateTime", "@value": v})
                };
                normalized.insert(term, json!({"type": "Property", "value": v}));
            }
        }
        if keeps(program, &Value::Object(normalized))? {
            out.push(Value::Object(simple));
        }
    }
    Ok(out)
}

/// Whether `program` keeps a state; none keeps every state.
fn keeps(program: Option<&Program>, state: &Value) -> Result<bool, Problem> {
    let Some(program) = program else {
        return Ok(true);
    };
    let truth = program.eval(state);
    let truth = truth.map_err(|_| bad(antares_ops::TOO_COMPLEX))?;
    Ok(truth == Truth::True)
}

/// A state as its Feature, its `properties` only those the request's `properties` keeps.
fn shaped(
    c: &Collection,
    terms: &Terms,
    state: &Value,
    refs: Refs<'_>,
    p: &Params,
) -> Map<String, Value> {
    let mut f = feature(c, terms, state, refs);
    if let (Some(keep), Some(Value::Object(props))) = (&p.properties, f.get_mut("properties")) {
        props.retain(|name, _| {
            c.properties
                .iter()
                .any(|prop| prop.name == *name && keep.contains(&prop.iri))
        });
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car() -> Collection {
        let prop = |name: &str, role: Option<&str>| Prop {
            name: name.to_owned(),
            iri: format!("https://example.org/{name}"),
            schema: json!({"type": "number"}),
            role: role.map(str::to_owned),
            unit: None,
            description: None,
            sortable: true,
            kind: "Property".to_owned(),
            reference: false,
            target: None,
        };
        Collection {
            history: false,
            output: None,
            id: "Car".to_owned(),
            description: None,
            type_iri: "https://example.org/Car".to_owned(),
            geometry: Some("https://uri.etsi.org/ngsi-ld/location".to_owned()),
            time: Some(Time::Instant("https://example.org/seen".to_owned())),
            properties: vec![prop("speed", None), prop("seen", Some("primary-instant"))],
        }
    }

    fn at(v: f64, t: &str) -> Value {
        json!({"type": "Property", "value": v, "observedAt": t})
    }

    fn here(x: f64, t: &str) -> Value {
        json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [x, 0.0]}, "observedAt": t})
    }

    /// Speed at 10:00 and 10:10, location at 10:00 and 10:05.
    fn entity() -> Value {
        json!({
            "id": "urn:ngsi-ld:Car:1", "type": "Car",
            "speed": [at(30.0, "2026-10-21T10:10:00Z"), at(10.0, "2026-10-21T10:00:00Z")],
            "location": [here(1.0, "2026-10-21T10:00:00Z"), here(2.0, "2026-10-21T10:05:00Z")]
        })
    }

    fn summary(states: &[Value]) -> Vec<(String, Value, Value, Value)> {
        states
            .iter()
            .map(|s| {
                (
                    s.get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    s.get("speed").cloned().unwrap_or_default(),
                    s.pointer("/location/coordinates/0")
                        .cloned()
                        .unwrap_or_default(),
                    s.get("validTo").cloned().unwrap_or_default(),
                )
            })
            .collect()
    }

    #[test]
    fn a_history_collection_reads_only_and_marks_its_interval() {
        let h = of(&car());
        assert_eq!(h.id, "Car-history");
        assert!(h.history);
        assert_eq!(
            h.time,
            Some(Time::Interval(
                Some(VALID_FROM.to_owned()),
                Some(VALID_TO.to_owned())
            ))
        );
        let roles: Vec<(&str, Option<&str>, bool)> = h
            .properties
            .iter()
            .map(|p| (p.name.as_str(), p.role.as_deref(), p.sortable))
            .collect();
        assert_eq!(
            roles,
            [
                ("speed", None, false),
                ("seen", None, false),
                ("entityId", None, false),
                ("validFrom", Some("primary-interval-start"), false),
                ("validTo", Some("primary-interval-end"), false),
            ]
        );
    }

    #[test]
    fn each_observed_at_is_a_state_with_the_values_valid_then() -> Result<(), Problem> {
        let h = of(&car());
        let terms = Terms::of(&h);
        let all = states(&h, &terms, &entity(), None, (None, None), None)?;
        assert_eq!(
            summary(&all),
            [
                (
                    "urn:ngsi-ld:Car:1@2026-10-21T10:00:00Z".to_owned(),
                    json!(10.0),
                    json!(1.0),
                    json!("2026-10-21T10:05:00Z")
                ),
                (
                    "urn:ngsi-ld:Car:1@2026-10-21T10:05:00Z".to_owned(),
                    json!(10.0),
                    json!(2.0),
                    json!("2026-10-21T10:10:00Z")
                ),
                (
                    "urn:ngsi-ld:Car:1@2026-10-21T10:10:00Z".to_owned(),
                    json!(30.0),
                    json!(2.0),
                    Value::Null
                ),
            ],
            "the location valid then is the latest at or before; the newest is open"
        );
        Ok(())
    }

    #[test]
    fn a_window_keeps_the_states_valid_in_it() -> Result<(), Problem> {
        let h = of(&car());
        let terms = Terms::of(&h);
        let micros = |t: &str| DateTime::parse(t).map(DateTime::unix_micros);
        let ids = |w: Window| -> Result<Vec<String>, Problem> {
            Ok(summary(&states(&h, &terms, &entity(), None, w, None)?)
                .into_iter()
                .map(|s| s.0.rsplit('@').next().unwrap_or_default().to_owned())
                .collect())
        };
        let instant = micros("2026-10-21T10:07:00Z");
        assert_eq!(ids((instant, instant))?, ["2026-10-21T10:05:00Z"]);
        let edge = micros("2026-10-21T10:05:00Z");
        assert_eq!(
            ids((edge, edge))?,
            ["2026-10-21T10:05:00Z"],
            "validTo is exclusive"
        );
        assert_eq!(
            ids((micros("2026-10-21T10:06:00Z"), None))?,
            ["2026-10-21T10:05:00Z", "2026-10-21T10:10:00Z"],
            "an open end keeps the newest"
        );
        assert_eq!(
            ids((None, micros("2026-10-21T09:00:00Z")))?,
            Vec::<String>::new()
        );
        Ok(())
    }

    #[test]
    fn a_dataset_instance_and_one_without_observed_at_are_no_state() {
        let e = json!({"speed": [
            {"type": "Property", "value": 1, "datasetId": "urn:x", "observedAt": "2026-10-21T10:00:00Z"},
            {"type": "Property", "value": 2},
            {"type": "Property", "value": 3, "observedAt": "2026-10-21T11:00:00+01:00"}
        ]});
        let got: Vec<(String, Value)> = instances(&e, "speed")
            .into_iter()
            .map(|(_, t, v)| (t, v))
            .collect();
        assert_eq!(got, [("2026-10-21T11:00:00+01:00".to_owned(), json!(3))]);
        let one = json!({"speed": {"type": "Property", "value": 4, "observedAt": "2026-10-21T10:00:00Z"}});
        assert_eq!(
            instances(&one, "speed").len(),
            1,
            "one instance may come unlisted"
        );
        assert_eq!(instances(&one, "other").len(), 0);
    }
}
