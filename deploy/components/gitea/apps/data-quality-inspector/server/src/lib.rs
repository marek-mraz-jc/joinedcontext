//! The server half of data-quality-inspector (T-3354): what the browser cannot keep. The page
//! inspects the Endpoint's data as it is now; the server runs the same inspection (`../wasm`)
//! once a day as the job `inspect-daily` (app.yaml `spec.server.jobs`, AP-154), through the App's
//! own Endpoint as the App's job principal, and keeps every run's scores in its own schema, so the
//! page shows the trend. Each run's full report, every finding of every entity, is a file under the
//! App's prefix. A run reads about thirty pages of the gateway, which takes far longer than a
//! request may (504 after 5 s, T-3579), so no request runs one.
//!
//! | Route | What it does |
//! |---|---|
//! | `GET /api/runs` | the kept runs, newest first, each with its scores per type; `stale` when the newest is over a day old |
//! | `GET /api/runs/{id}/report` | a URL to download the run's full per-entity report from |
//!
//! Roles and edits (T-3827, API/07), decided from `identity::caller()` alone; the Endpoint's
//! Policy grants `updateAttrs` to the steward role only, the second check of a correction:
//!
//! | Route | Who | What it does |
//! |---|---|---|
//! | `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
//! | `GET /api/resolutions` | anyone | the findings stewards resolved, newest first |
//! | `POST /api/resolutions` `{entity, attribute, note}` | steward | a finding marked resolved |
//! | `DELETE /api/resolutions/{id}` | steward | the finding open again |
//! | `POST /api/corrections` `{entity, attribute, value, note}` | steward | one attribute of the entity corrected through the Endpoint, the finding resolved |
//! | `GET /api/history` | signed in | the newest 100 changes (a viewer: their own) |

use std::collections::BTreeMap;

use data_quality_inspector::{inspect_type_with, TypeInput, TypeOutput};
use jc_app_sdk::access::{self, Owned};
use jc_app_sdk::blob::{self, Method};
use jc_app_sdk::gateway;
use jc_app_sdk::http::{Params, Request, Response, Router};
use jc_app_sdk::identity::{self, Caller};
use jc_app_sdk::sql::{self, Value};
use serde::Deserialize;
use serde_json::{json, Map, Value as Json};

/// How long a presigned URL lives; the host allows at most 300 seconds.
const URL_SECONDS: u32 = 120;
/// Entities read per page, and at most per type: a run reads at most 27 pages of the twelve types,
/// about 35 s on dev, inside a job's 60 s wall time (T-3579).
const PAGE: usize = 500;
const MOST: usize = 2000;
/// Runs answered at most, for the trend.
const RUNS: i64 = 100;
/// The types the App's data need names, inspected without a schema when none is published.
const KNOWN_TYPES: [&str; 12] = [
    "AdministrativeArea",
    "AirQualityObserved",
    "Alert",
    "Article",
    "BikeHireDockingStation",
    "CityWork",
    "Event",
    "OnStreetParking",
    "PointOfInterest",
    "Vehicle",
    "WaterQualityObserved",
    "WeatherObserved",
];

/// The App role that resolves findings and corrects attributes (app.yaml).
const STEWARD: &str = "steward";
/// A resolution any steward may take back.
const RESOLUTIONS: Owned = Owned::new("resolutions", STEWARD);
/// Resolutions answered at most, and the history read.
const RESOLUTIONS_LISTED: i64 = 500;
const HISTORY: i64 = 100;
/// Attributes a correction never writes: the entity's identity and its geometry, which is no
/// plain Property.
const NOT_CORRECTED: [&str; 4] = ["id", "type", "location", "@context"];

/// A finding's entity and attribute: an NGSI-LD URN of a type the App reads, and an attribute name
/// a correction may write; or why not.
pub fn finding_target(entity: &str, attribute: &str) -> Result<(), String> {
    let kind = entity
        .strip_prefix("urn:ngsi-ld:")
        .and_then(|rest| rest.split_once(':'))
        .filter(|(kind, nss)| KNOWN_TYPES.contains(kind) && !nss.is_empty())
        .map(|(kind, _)| kind);
    if kind.is_none() || entity.len() > 256 || entity.chars().any(char::is_control) {
        return Err(format!("{entity} is not an entity this App reads"));
    }
    let mut chars = attribute.chars();
    let named = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && attribute.len() <= 64;
    if !named || NOT_CORRECTED.contains(&attribute) {
        return Err(format!(
            "{attribute} is not an attribute a steward corrects"
        ));
    }
    Ok(())
}

/// A note, trimmed, of 1 to 200 characters on one line; or why not.
pub fn note(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 200 || text.chars().any(char::is_control) {
        return Err("a note is 1 to 200 characters, on one line".to_owned());
    }
    Ok(text.to_owned())
}

/// A corrected value: a text of 1 to 500 characters on one line (trimmed), a finite number or a
/// yes/no; or why not. A list, an object or nothing is no correction of one attribute.
pub fn corrected_value(value: &Json) -> Result<Json, String> {
    match value {
        Json::Bool(_) => Ok(value.clone()),
        Json::Number(n) if n.as_f64().is_some_and(f64::is_finite) => Ok(value.clone()),
        Json::String(text) => {
            let text = text.trim();
            if text.is_empty() || text.chars().count() > 500 || text.chars().any(char::is_control) {
                Err("a text value is 1 to 500 characters, on one line".to_owned())
            } else {
                Ok(Json::String(text.to_owned()))
            }
        }
        _ => Err("a value is a text, a number or true or false".to_owned()),
    }
}

/// A keyValues attribute as the browser's SDK reads it (`cell`): a DateTime's `@value`, a
/// language map's English or first text, a geometry as itself, a list as joined text.
pub fn cell(value: &Json) -> Json {
    match value {
        Json::Null | Json::Bool(_) | Json::Number(_) | Json::String(_) => value.clone(),
        Json::Array(items) => Json::String(
            items
                .iter()
                .map(|item| match cell(item) {
                    Json::String(text) => text,
                    Json::Null => String::new(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(", "),
        ),
        Json::Object(object) => {
            if let Some(inner) = object.get("@value") {
                return cell(inner);
            }
            if let Some(Json::Object(map)) = object.get("languageMap") {
                let picked = map.get("en").or_else(|| map.values().next());
                return picked.map_or(Json::Null, cell);
            }
            if object.get("type").is_some_and(Json::is_string) && object.contains_key("coordinates")
            {
                return json!({"type": object["type"], "coordinates": object["coordinates"]});
            }
            if let Some(inner) = object.get("value").or_else(|| object.get("object")) {
                return cell(inner);
            }
            match object.values().find(|v| v.is_string()) {
                Some(text) => text.clone(),
                None => Json::String(value.to_string()),
            }
        }
    }
}

/// An entity as the browser's row: every attribute through [`cell`], `@context` left out.
pub fn row(entity: &Json) -> Json {
    let mut row = Map::new();
    if let Some(object) = entity.as_object() {
        for (key, value) in object {
            if key != "@context" {
                row.insert(key.clone(), cell(value));
            }
        }
    }
    Json::Object(row)
}

/// The published schema's types and their definitions, as the browser's SDK merges them: every
/// model's `definitions` or `$defs`, only the types the index says are served, no `Entity`.
pub fn served_schema(index: &Json, documents: &[Json]) -> BTreeMap<String, Json> {
    let served: Vec<&str> = index["models"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|m| {
            m["types"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
        })
        .collect();
    let mut merged = BTreeMap::new();
    for document in documents {
        for key in ["$defs", "definitions"] {
            if let Some(defs) = document.get(key).and_then(Json::as_object) {
                for (name, definition) in defs {
                    merged.insert(name.clone(), definition.clone());
                }
            }
        }
    }
    merged.retain(|name, definition| {
        name != "Entity"
            && (served.is_empty() || served.contains(&name.as_str()))
            && definition.get("properties").is_some()
    });
    merged
}

/// The run's scores over every type, weighted by the entities each holds; `valid` only over the
/// types with a published schema.
pub fn overall(types: &[TypeOutput]) -> (usize, usize, f64, Option<f64>) {
    let entities: usize = types.iter().map(|t| t.entities).sum();
    let findings: usize = types.iter().map(|t| t.findings_total).sum();
    let weighted = |pick: &dyn Fn(&TypeOutput) -> Option<f64>| {
        let (sum, n) = types
            .iter()
            .fold((0.0, 0usize), |(sum, n), t| match pick(t) {
                Some(score) if t.entities > 0 => (sum + score * t.entities as f64, n + t.entities),
                _ => (sum, n),
            });
        (n > 0).then(|| sum / n as f64)
    };
    let completeness = weighted(&|t| Some(t.completeness)).unwrap_or(0.0);
    (entities, findings, completeness, weighted(&|t| t.valid))
}

/// A run's full report: every type's findings grouped by the entity they are about.
pub fn report(types: &[TypeOutput]) -> Json {
    json!({
        "types": types.iter().map(|t| {
            let mut by_entity: BTreeMap<&str, Vec<Json>> = BTreeMap::new();
            for f in &t.findings {
                by_entity.entry(f.entity.as_str()).or_default().push(json!({"attribute": f.attribute, "rule": f.rule, "detail": f.detail}));
            }
            json!({
                "type": t.entity_type,
                "entities": t.entities,
                "completeness": t.completeness,
                "valid": t.valid,
                "attributes": t.attributes,
                "freshness": t.freshness,
                "findings": by_entity.into_iter().map(|(entity, findings)| json!({"entity": entity, "findings": findings})).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
    })
}

/// Every entity of `kind` the App's Endpoint lets the caller read, page by page, up to [`MOST`].
fn entities(kind: &str) -> Result<Vec<Json>, gateway::Error> {
    let mut all = Vec::new();
    loop {
        let path = format!(
            "/ngsi-ld/v1/entities?type={}&options=keyValues&limit={PAGE}&offset={}",
            gateway::encode(kind),
            all.len()
        );
        let page: Vec<Json> = gateway::get_json(&path)?;
        let last = page.len() < PAGE;
        all.extend(page);
        if last || all.len() >= MOST {
            all.truncate(MOST);
            return Ok(all);
        }
    }
}

/// The Endpoint's published schema, or `None` when it publishes none the App can read.
fn schema() -> Option<BTreeMap<String, Json>> {
    let index: Json = gateway::get_json("/schema/index.json").ok()?;
    let mut versions: Vec<i64> = index["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["version"].as_i64())
        .collect();
    versions.sort_unstable();
    versions.dedup();
    if versions.is_empty() {
        versions.push(1);
    }
    let documents: Vec<Json> = versions
        .iter()
        .filter_map(|v| gateway::get_json(&format!("/schema/v{v}/json-schema")).ok())
        .collect();
    let schema = served_schema(&index, &documents);
    (!schema.is_empty()).then_some(schema)
}

/// The daily job `inspect-daily` (app.yaml `spec.server.jobs`, AP-154): one run, or the sentence
/// that says what stopped it (AP-155), never a body or a credential.
pub fn inspect_daily() -> Result<(), String> {
    inspect().map(|_| ())
}

/// One run now: every type read and inspected, its scores kept, its full report written.
fn inspect() -> Result<i64, String> {
    let schema = schema();
    let names: Vec<String> = match &schema {
        Some(schema) => schema.keys().cloned().collect(),
        None => KNOWN_TYPES.iter().map(|t| (*t).to_owned()).collect(),
    };
    let now = sql::query("select extract(epoch from now())::float8", &[])
        .ok()
        .and_then(
            |rows| match rows.values.first().and_then(|row| row.first()) {
                Some(Value::Float(now)) => Some(*now),
                _ => None,
            },
        )
        .ok_or("the database did not say what time it is")?;
    let mut types = Vec::new();
    for name in names {
        let rows = entities(&name)
            .map_err(|err| format!("{name} could not be read: {err}"))?
            .iter()
            .map(row)
            .collect();
        let input = TypeInput {
            entity_type: name.clone(),
            schema: schema.as_ref().and_then(|s| s.get(&name).cloned()),
            rows,
        };
        types.push(inspect_type_with(now, &input, usize::MAX));
    }
    let (entities, findings, completeness, valid) = overall(&types);
    let rows = sql::query(
        "insert into quality_runs (types, entities, findings, completeness, valid) values ($1, $2, $3, $4, $5::float8) returning id",
        &[
            Value::from(types.len() as i64),
            Value::from(entities as i64),
            Value::from(findings as i64),
            Value::from(completeness),
            Value::from(valid),
        ],
    )
    .map_err(|err| format!("the run could not be kept: {err}"))?;
    let Some(Value::Int(id)) = rows.values.first().and_then(|row| row.first()).cloned() else {
        return Err("the run was not given an id".into());
    };
    for t in &types {
        sql::execute(
            "insert into run_types (run_id, type, entities, completeness, valid, findings, freshness_median_seconds) \
             values ($1, $2, $3, $4, $5::float8, $6, $7::float8)",
            &[
                Value::from(id),
                Value::from(t.entity_type.clone()),
                Value::from(t.entities as i64),
                Value::from(t.completeness),
                Value::from(t.valid),
                Value::from(t.findings_total as i64),
                Value::from(t.freshness.as_ref().map(|f| f.median_seconds)),
            ],
        )
        .map_err(|err| format!("the scores of {} could not be kept: {err}", t.entity_type))?;
    }
    blob::put(
        &report_key(id),
        report(&types).to_string().as_bytes(),
        Some("application/json"),
    )
    .map_err(|err| format!("the report could not be written: {err}"))?;
    Ok(id)
}

fn report_key(id: i64) -> String {
    format!("runs/{id}/report.json")
}

/// Whether the newest run is younger than `interval` (a Postgres interval), or there is one at all.
fn ran_within(interval: &str) -> Result<bool, Response> {
    let rows = sql::query(
        "select count(*) from quality_runs where ran_at > now() - $1::interval",
        &[Value::from(interval)],
    )
    .map_err(Response::from_sql)?;
    Ok(!matches!(
        rows.values.first().and_then(|row| row.first()),
        Some(Value::Int(0)) | None
    ))
}

fn runs(_: &Request, _: &Params) -> Response {
    // The daily job keeps a point a day; a day without one (the job failed, or has not run yet)
    // leaves the kept runs readable, and says so.
    let stale = match ran_within("1 day") {
        Ok(ran) => !ran,
        Err(answer) => return answer,
    };
    let runs = match sql::query(
        r#"select id, to_char(ran_at at time zone 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as ran_at, types, entities, findings,
                  completeness::float8 as completeness, valid::float8 as valid
           from quality_runs order by id desc limit $1"#,
        &[Value::from(RUNS)],
    ) {
        Ok(rows) => sql::objects(&rows),
        Err(err) => return Response::from_sql(err),
    };
    let types = match sql::query(
        "select run_id, type, entities, completeness::float8 as completeness, valid::float8 as valid, findings, freshness_median_seconds \
         from run_types where run_id in (select id from quality_runs order by id desc limit $1) order by run_id desc, type",
        &[Value::from(RUNS)],
    ) {
        Ok(rows) => sql::objects(&rows),
        Err(err) => return Response::from_sql(err),
    };
    let mut by_run: BTreeMap<i64, Vec<Json>> = BTreeMap::new();
    for mut t in types {
        let run = t
            .remove("run_id")
            .and_then(|r| r.as_i64())
            .unwrap_or_default();
        by_run.entry(run).or_default().push(Json::Object(t));
    }
    let runs: Vec<Json> = runs
        .into_iter()
        .map(|mut run| {
            let id = run.get("id").and_then(Json::as_i64).unwrap_or_default();
            run.insert(
                "types".into(),
                Json::Array(by_run.remove(&id).unwrap_or_default()),
            );
            Json::Object(run)
        })
        .collect();
    Response::json(200, &json!({ "runs": runs, "stale": stale }))
}

fn download(_: &Request, params: &Params) -> Response {
    let Ok(id) = params["id"].parse::<i64>() else {
        return Response::problem(404, "Not Found", "no such run");
    };
    match sql::query(
        "select 1 from quality_runs where id = $1",
        &[Value::from(id)],
    ) {
        Ok(rows) if rows.values.is_empty() => Response::problem(404, "Not Found", "no such run"),
        Ok(_) => match blob::presign(&report_key(id), Method::Get, URL_SECONDS) {
            Ok(url) => Response::json(200, &json!({ "url": url })),
            Err(err) => Response::from_blob(err),
        },
        Err(err) => Response::from_sql(err),
    }
}

fn me(_: &Request, _: &Params) -> Response {
    match identity::caller() {
        Some(caller) => Response::json(
            200,
            &json!({"id": caller.id, "name": caller.name, "roles": caller.roles}),
        ),
        None => Response::json(200, &Json::Null),
    }
}

fn bad(why: &str) -> Response {
    Response::problem(400, "Bad Request", why)
}

/// The signed-in steward, or the `401`/`403` that says why not.
fn steward() -> Result<Caller, Response> {
    let caller = Caller::require()?;
    caller.require_role(&[STEWARD])?;
    Ok(caller)
}

fn resolutions(_: &Request, _: &Params) -> Response {
    match sql::query(
        "select id, entity, attribute, note, corrected, resolver_name as \"resolverName\", \
         created_at as \"resolvedAt\" from resolutions order by id desc limit $1",
        &[Value::Int(RESOLUTIONS_LISTED)],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

/// Whether the finding is resolved already.
fn resolved(entity: &str, attribute: &str) -> Result<bool, Response> {
    sql::query(
        "select 1 as one from resolutions where entity = $1 and attribute = $2 limit 1",
        &[Value::from(entity), Value::from(attribute)],
    )
    .map(|rows| !rows.values.is_empty())
    .map_err(Response::from_sql)
}

/// The finding marked resolved by `caller`; the new row as the page reads it.
fn resolve(
    caller: &Caller,
    entity: &str,
    attribute: &str,
    note: &str,
    corrected: bool,
) -> Response {
    match RESOLUTIONS.insert(
        caller,
        &["entity", "attribute", "note", "corrected", "resolver_name"],
        vec![
            Value::from(entity),
            Value::from(attribute),
            Value::from(note),
            Value::Boolean(corrected),
            Value::from(caller.name.as_str()),
        ],
        &format!("resolved {attribute} of {entity}: {note}"),
    ) {
        Ok(rows) => {
            let row = sql::objects(&rows).into_iter().next().unwrap_or_default();
            Response::json(
                201,
                &json!({"id": row.get("id"), "entity": entity, "attribute": attribute, "note": note, "corrected": corrected, "resolverName": caller.name, "resolvedAt": row.get("created_at")}),
            )
        }
        Err(answer) => answer,
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Resolve {
    entity: String,
    attribute: String,
    note: String,
}

fn mark_resolved(request: &Request, _: &Params) -> Response {
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let wanted: Resolve = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let checked =
        finding_target(&wanted.entity, &wanted.attribute).and_then(|()| note(&wanted.note));
    let note = match checked {
        Ok(note) => note,
        Err(why) => return bad(&why),
    };
    match resolved(&wanted.entity, &wanted.attribute) {
        Ok(true) => Response::problem(409, "Conflict", "this finding is resolved already"),
        Ok(false) => resolve(&caller, &wanted.entity, &wanted.attribute, &note, false),
        Err(answer) => answer,
    }
}

fn reopen(_: &Request, params: &Params) -> Response {
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let Ok(id) = params["id"].parse::<i64>() else {
        return Response::problem(404, "Not Found", "no such resolution");
    };
    match RESOLUTIONS.delete(&caller, id, "reopened a finding") {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Correct {
    entity: String,
    attribute: String,
    value: Json,
    note: String,
}

fn correct(request: &Request, _: &Params) -> Response {
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let wanted: Correct = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let checked = finding_target(&wanted.entity, &wanted.attribute)
        .and_then(|()| corrected_value(&wanted.value))
        .and_then(|value| note(&wanted.note).map(|note| (value, note)));
    let (value, note) = match checked {
        Ok(checked) => checked,
        Err(why) => return bad(&why),
    };
    // The Endpoint's Policy is the second check: a person it does not let write is refused there.
    let attrs = json!({ wanted.attribute.as_str(): {"type": "Property", "value": value} });
    if let Err(err) = gateway::patch_entity(&wanted.entity, &attrs) {
        return Response::from(err);
    }
    if let Err(answer) = access::record(
        &caller,
        "edit",
        &wanted.entity,
        &format!("corrected {}: {note}", wanted.attribute),
    ) {
        return answer;
    }
    match resolved(&wanted.entity, &wanted.attribute) {
        Ok(true) => Response::json(
            200,
            &json!({"entity": wanted.entity, "attribute": wanted.attribute, "corrected": true}),
        ),
        Ok(false) => resolve(&caller, &wanted.entity, &wanted.attribute, &note, true),
        Err(answer) => answer,
    }
}

fn changes(_: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    match sql::query(
        "select id, at, actor_name as \"actorName\", action, target, summary from app_audit \
         where actor_id = $1 or $2::boolean order by at desc, id desc limit $3",
        &[
            Value::from(caller.id.as_str()),
            Value::Boolean(caller.has_role(STEWARD)),
            Value::Int(HISTORY),
        ],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

pub fn handle(request: Request) -> Response {
    Router::new()
        .get("/api/runs", runs)
        .get("/api/runs/{id}/report", download)
        .get("/api/me", me)
        .get("/api/resolutions", resolutions)
        .post("/api/resolutions", mark_resolved)
        .delete("/api/resolutions/{id}", reopen)
        .post("/api/corrections", correct)
        .get("/api/history", changes)
        .handle(&request)
}

jc_app_sdk::app!(handle);

// The job's export, a root-level `inspect-daily: func() -> result<_, string>` beside the request
// handler, as `wit/server.wit` declares it (AP-159).
#[cfg(target_arch = "wasm32")]
mod job {
    wit_bindgen::generate!({ path: "wit", world: "helsinki:data-quality-inspector/server", generate_all });

    struct Jobs;

    impl Guest for Jobs {
        fn inspect_daily() -> Result<(), String> {
            super::inspect_daily()
        }
    }

    export!(Jobs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use data_quality_inspector::Finding;

    /// T-3827: a correction names an entity of a type the App reads and an attribute a steward may
    /// write, never the identity or the geometry.
    #[test]
    fn a_correction_names_an_entity_the_app_reads_and_a_plain_attribute() {
        let alert = "urn:ngsi-ld:Alert:hel.fi:helsinki:a-1";
        assert_eq!(finding_target(alert, "description"), Ok(()));
        assert_eq!(finding_target(alert, "valid_to2"), Ok(()));
        for attribute in [
            "id",
            "type",
            "location",
            "@context",
            "",
            "2name",
            "a-b",
            &"a".repeat(65),
        ] {
            assert!(finding_target(alert, attribute).is_err(), "{attribute}");
        }
        for entity in [
            "urn:ngsi-ld:Building:x",
            "urn:ngsi-ld:Alert:",
            "Alert:x",
            "urn:ngsi-ld:Alert:a\nb",
            &format!("urn:ngsi-ld:Alert:{}", "x".repeat(250)),
        ] {
            assert!(finding_target(entity, "name").is_err(), "{entity}");
        }
    }

    #[test]
    fn a_corrected_value_is_a_text_a_number_or_a_yes_no() {
        assert_eq!(corrected_value(&json!("  Kallio ")), Ok(json!("Kallio")));
        assert_eq!(corrected_value(&json!(12.5)), Ok(json!(12.5)));
        assert_eq!(corrected_value(&json!(false)), Ok(json!(false)));
        for value in [
            json!(""),
            json!("   "),
            json!("a\nb"),
            json!("x".repeat(501)),
            json!(null),
            json!([1]),
            json!({"a": 1}),
        ] {
            assert!(corrected_value(&value).is_err(), "{value}");
        }
        assert_eq!(note("  checked on site "), Ok("checked on site".to_owned()));
        assert!(note("").is_err());
        assert!(note("two\nlines").is_err());
        assert!(note(&"x".repeat(201)).is_err());
    }

    #[test]
    fn a_cell_is_read_as_the_browser_reads_it() {
        assert_eq!(
            cell(&json!({"@type": "DateTime", "@value": "2026-10-08T07:00:00Z"})),
            json!("2026-10-08T07:00:00Z")
        );
        assert_eq!(
            cell(&json!({"languageMap": {"fi": "Kirjasto", "en": "Library"}})),
            json!("Library")
        );
        assert_eq!(
            cell(&json!({"languageMap": {"fi": "Kirjasto"}})),
            json!("Kirjasto")
        );
        assert_eq!(
            cell(&json!({"type": "Point", "coordinates": [24.9, 60.1], "extra": 1})),
            json!({"type": "Point", "coordinates": [24.9, 60.1]})
        );
        assert_eq!(
            cell(&json!({"object": "urn:ngsi-ld:AdministrativeArea:x"})),
            json!("urn:ngsi-ld:AdministrativeArea:x")
        );
        assert_eq!(cell(&json!(["a", 1, null])), json!("a, 1, "));
        assert_eq!(cell(&json!({"n": 1})), json!("{\"n\":1}"));
        assert_eq!(cell(&json!(4.5)), json!(4.5));
        let r = row(&json!({"@context": "x", "id": "urn:a", "name": {"languageMap": {"en": "A"}}}));
        assert_eq!(r, json!({"id": "urn:a", "name": "A"}));
    }

    #[test]
    fn the_schema_is_merged_as_the_sdk_merges_it() {
        let index = json!({"models": [{"version": 1, "types": ["Alert", "Event"]}]});
        let documents = [
            json!({"definitions": {"Entity": {"properties": {}}, "Alert": {"properties": {"name": {}}}, "Vehicle": {"properties": {}}}}),
            json!({"$defs": {"Event": {"properties": {"startDate": {}}}, "Hidden": {"type": "object"}}}),
        ];
        let merged = served_schema(&index, &documents);
        assert_eq!(merged.keys().collect::<Vec<_>>(), ["Alert", "Event"]);
        assert!(served_schema(&json!({}), &[]).is_empty());
    }

    fn output(
        name: &str,
        entities: usize,
        completeness: f64,
        valid: Option<f64>,
        findings: Vec<Finding>,
    ) -> TypeOutput {
        TypeOutput {
            entity_type: name.into(),
            entities,
            completeness,
            valid,
            attributes: vec![],
            findings_total: findings.len(),
            findings,
            freshness: None,
        }
    }

    fn finding(entity: &str, attribute: &str) -> Finding {
        Finding {
            entity: entity.into(),
            attribute: attribute.into(),
            rule: "type".into(),
            detail: "not a number".into(),
        }
    }

    #[test]
    fn a_runs_scores_are_weighted_by_entities_and_valid_only_where_a_schema_is() {
        let types = [
            output(
                "Alert",
                3,
                1.0,
                Some(0.5),
                vec![finding("a1", "x"), finding("a1", "y"), finding("a2", "x")],
            ),
            output("Event", 1, 0.0, None, vec![]),
            output("Empty", 0, 0.0, Some(0.0), vec![]),
        ];
        let (entities, findings, completeness, valid) = overall(&types);
        assert_eq!((entities, findings), (4, 3));
        assert!((completeness - 0.75).abs() < 1e-9);
        assert_eq!(valid, Some(0.5));
        assert_eq!(overall(&[]), (0, 0, 0.0, None));
        let full = report(&types);
        assert_eq!(
            full["types"][0]["findings"][0],
            json!({"entity": "a1", "findings": [
            {"attribute": "x", "rule": "type", "detail": "not a number"}, {"attribute": "y", "rule": "type", "detail": "not a number"}]})
        );
        assert_eq!(
            full["types"][0]["findings"].as_array().map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn a_route_outside_the_api_or_a_bad_id_is_answered_in_words() {
        let get = |path: &str| {
            handle(Request {
                method: "GET".into(),
                path: path.into(),
                ..Request::default()
            })
        };
        assert_eq!(get("/api/nothing").status, 404);
        assert_eq!(get("/api/runs/abc/report").status, 404);
        let wrong = handle(Request {
            method: "DELETE".into(),
            path: "/api/runs".into(),
            ..Request::default()
        });
        assert_eq!(wrong.status, 405);
    }
}
