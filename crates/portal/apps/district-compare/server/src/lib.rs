//! The server half of district-compare (T-3353): what the browser cannot keep. The feeds hold
//! today's events, stations, alerts and readings, so once a day on Helsinki's calendar the job
//! `keep-metrics` reads them through the App's own Endpoint as the App's job principal, runs the browser's own
//! comparison (`../wasm`) and keeps each district's figures for that day in its own schema: a
//! district gets a history. It also keeps the district boundaries it compared against as a
//! GeoJSON file with its licence under the App's prefix. Choosing districts and a measure stays in
//! the browser.
//!
//! | Route | What it does |
//! |---|---|
//! | `GET /api/metrics?codes=101,102` | the kept days of up to six districts, the newest first; `stale` until the job kept today's |
//! | `GET /api/boundaries` | URLs to download the boundaries and their licence from |
//!
//! Roles and edits (T-3828, API/07), decided from `identity::caller()` alone: a signed-in reader
//! keeps comparisons of their own and shares one by its link, which any signed-in reader of the
//! project opens read-only; a steward pins the comparison everyone opens first. The link carries
//! the comparison's share token (64 hex characters the database draws), never its id: an id is a
//! bigserial anyone could count through, so by id a comparison answers its owner only.
//!
//! | Route | Who | What it does |
//! |---|---|---|
//! | `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
//! | `GET /api/comparisons` | signed in | the caller's comparisons, newest first |
//! | `POST /api/comparisons` `{name, codes, measure}` | signed in | a comparison kept |
//! | `GET /api/comparisons/{id}` | owner | the caller's own comparison |
//! | `GET /api/shared/{share}` | signed in | the comparison a link's token names, read-only, with its owner's name |
//! | `PUT /api/comparisons/{id}` `{name, codes, measure}` | owner | changed |
//! | `DELETE /api/comparisons/{id}` | owner | gone |
//! | `GET /api/default` | anyone | the pinned comparison, `null` when none |
//! | `PUT /api/default` `{name, codes, measure}` | steward | pinned for everyone |
//! | `DELETE /api/default` | steward | the newest pin taken back |
//! | `GET /api/history` | signed in | the newest changes (a viewer: their own) |

use district_compare::{run, AirInput, BikeInput, DistrictInput, Input, Output};
use jc_app_sdk::access::Owned;
use jc_app_sdk::blob::{self, Method};
use jc_app_sdk::gateway;
use jc_app_sdk::http::{Params, Request, Response, Router};
use jc_app_sdk::identity::{self, Caller};
use jc_app_sdk::sql::{self, Value};
use serde::Deserialize;
use serde_json::{json, Value as Json};

/// How long a presigned URL lives; the host allows at most 300 seconds.
const URL_SECONDS: u32 = 120;
/// Entities read from the feed per page, and at most per type.
const PAGE: usize = 500;
const MOST: usize = 5000;
/// Districts compared at once, as the page allows, and days answered per district.
const MOST_CODES: usize = 6;
const DAYS: i64 = 120;
const STEWARD: &str = "steward";
const COMPARISONS: Owned = Owned::new("comparisons", "");
const PINS: Owned = Owned::new("pins", STEWARD);
/// Comparisons a reader keeps at most, and changes answered in History.
const MOST_COMPARISONS: i64 = 20;
const HISTORY: i64 = 100;
/// The measures the page offers (`src/districts.ts`).
pub const MEASURES: [&str; 6] = ["events", "bikes", "bikeSlots", "alerts", "pm25", "aqi"];
const BOUNDARIES: &str = "boundaries/districts.geojson";
const LICENCE: &str = "boundaries/LICENCE.txt";
/// Where the boundaries come from and under which terms: the platform's district pipeline reads
/// them from this layer, published under CC BY 4.0 (deployment's helsinki-datasource-districts).
const LICENCE_TEXT: &str = "Helsinki's districts (kaupunginosajako)\n\n\
Source: City of Helsinki, the open WFS layer avoindata:Kaupunginosajako\n\
(https://kartta.hel.fi/ws/geoserver/avoindata/wfs), listed in the Helsinki Region Infoshare\n\
catalogue as the package helsingin-piirijako.\n\
Licence: Creative Commons Attribution 4.0 International (CC BY 4.0),\n\
https://creativecommons.org/licenses/by/4.0/\n\n\
This file holds the boundaries as the joinedcontext platform's AdministrativeArea entities carried them\n\
on the day written below; the App district-compare wrote it.\n";

/// A keyValues text as a person reads it: a string, the Finnish or English of a language map.
pub fn text_of(value: Option<&Json>) -> String {
    match value {
        Some(Json::String(text)) => text.clone(),
        Some(Json::Number(number)) => number.to_string(),
        Some(Json::Object(map)) => ["fi", "en", "sv"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Json::as_str))
            .or_else(|| map.values().find_map(Json::as_str))
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

/// Where an entity is, as the browser places it: a Point's coordinates, else the first vertex of
/// a line or polygon; `None` when it has no place.
pub fn point_of(value: Option<&Json>) -> Option<Vec<f64>> {
    let mut coordinates = value?.get("coordinates")?;
    while let Some(first) = coordinates
        .as_array()
        .and_then(|c| c.first())
        .filter(|c| c.is_array())
    {
        coordinates = first;
    }
    let pair = coordinates.as_array()?;
    Some(vec![pair.first()?.as_f64()?, pair.get(1)?.as_f64()?])
}

/// A number, or a number written as text.
pub fn number_of(value: Option<&Json>) -> Option<f64> {
    match value? {
        Json::Number(number) => number.as_f64(),
        Json::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// The feeds' entities as the comparison takes them, as the browser builds its input: districts
/// of the level `district` only, an entity with no place counted outside every district.
pub fn input(
    districts: &[Json],
    events: &[Json],
    bikes: &[Json],
    alerts: &[Json],
    air: &[Json],
) -> Input {
    let nowhere = || vec![999.0, 999.0];
    Input {
        districts: districts
            .iter()
            .filter(|d| d.get("divisionLevel").and_then(Json::as_str) == Some("district"))
            .map(|d| {
                let code = text_of(d.get("districtCode"));
                let name = text_of(d.get("name"));
                DistrictInput {
                    name: if name.trim().is_empty() || name.starts_with("urn:") {
                        code.clone()
                    } else {
                        name
                    },
                    code,
                    geometry: d.get("location").cloned().unwrap_or(Json::Null),
                }
            })
            .collect(),
        events: events
            .iter()
            .map(|e| point_of(e.get("location")).unwrap_or_else(nowhere))
            .collect(),
        bikes: bikes
            .iter()
            .map(|b| BikeInput {
                at: Some(point_of(b.get("location")).unwrap_or_else(nowhere)),
                slots: number_of(b.get("totalSlotNumber")).map(|n| n as i64),
            })
            .collect(),
        alerts: alerts
            .iter()
            .map(|a| point_of(a.get("location")).unwrap_or_else(nowhere))
            .collect(),
        air: air
            .iter()
            .map(|a| AirInput {
                at: Some(point_of(a.get("location")).unwrap_or_else(nowhere)),
                pm25: number_of(a.get("pm25")),
                aqi: number_of(a.get("airQualityIndex")),
            })
            .collect(),
    }
}

/// The districts the comparison ran on as a GeoJSON FeatureCollection, each with its code and name.
pub fn boundaries(input: &Input) -> Json {
    json!({
        "type": "FeatureCollection",
        "features": input.districts.iter().filter(|d| d.geometry.is_object()).map(|d| json!({
            "type": "Feature",
            "properties": {"code": d.code, "name": d.name},
            "geometry": d.geometry,
        })).collect::<Vec<_>>(),
    })
}

/// The codes a request asks for: 1 to [`MOST_CODES`], each 1 to 20 letters, digits, `-` or `_`.
pub fn codes(raw: Option<&str>) -> Result<Vec<String>, String> {
    let codes: Vec<String> = raw
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
        .collect();
    if codes.is_empty() || codes.len() > MOST_CODES {
        return Err(format!(
            "name 1 to {MOST_CODES} districts by their codes: ?codes=101,102"
        ));
    }
    if codes.iter().any(|c| {
        c.len() > 20
            || !c
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }) {
        return Err("a district's code is 1 to 20 letters, digits, dashes or underscores".into());
    }
    Ok(codes)
}

/// A comparison as a reader names it: the districts and the measure the page shows.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewComparison {
    pub name: String,
    pub codes: Vec<String>,
    pub measure: String,
}

/// The comparison's name and its codes as kept (`101,102`), or why not.
pub fn checked_comparison(new: &NewComparison) -> Result<(String, String), String> {
    let name = new.name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("a comparison's name is one line of 1 to 80 characters".into());
    }
    if new.codes.iter().any(|c| c.contains(',')) {
        return Err("a district's code is 1 to 20 letters, digits, dashes or underscores".into());
    }
    let mut kept = codes(Some(&new.codes.join(",")))?;
    let before = kept.len();
    kept.sort();
    kept.dedup();
    if kept.len() != before {
        return Err("each district is compared once".into());
    }
    if !MEASURES.contains(&new.measure.as_str()) {
        return Err(format!("the measure is one of {}", MEASURES.join(", ")));
    }
    Ok((name.to_owned(), new.codes.join(",")))
}

/// Every entity of `kind` the App's Endpoint lets the caller read, page by page, up to [`MOST`].
fn entities(kind: &str, attrs: &str) -> Result<Vec<Json>, gateway::Error> {
    let mut all = Vec::new();
    loop {
        let path = format!(
            "/ngsi-ld/v1/entities?type={kind}&options=keyValues&limit={PAGE}&offset={}&attrs={}",
            all.len(),
            gateway::encode(attrs),
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

/// Reads the feeds, compares, and keeps today's figures of every district and the boundaries.
fn refresh() -> Result<(), String> {
    let read =
        |kind: &str, attrs: &str| entities(kind, attrs).map_err(|err| format!("{kind}: {err}"));
    let districts = read(
        "AdministrativeArea",
        "name,districtCode,divisionLevel,location",
    )?;
    let input = input(
        &districts,
        &read("Event", "location")?,
        &read("BikeHireDockingStation", "totalSlotNumber,location")?,
        &read("Alert", "location")?,
        &read("AirQualityObserved", "pm25,airQualityIndex,location")?,
    );
    let output: Output = run(&input);
    for d in &output.districts {
        sql::execute(
            "insert into district_metrics (day, code, name, area_km2, events, bikes, bike_slots, alerts, pm25, aqi, computed_at) \
             values ((now() at time zone 'Europe/Helsinki')::date, $1, $2, $3, $4, $5, $6, $7, $8::float8, $9::float8, now()) \
             on conflict (day, code) do update set name = excluded.name, area_km2 = excluded.area_km2, events = excluded.events, \
             bikes = excluded.bikes, bike_slots = excluded.bike_slots, alerts = excluded.alerts, pm25 = excluded.pm25, \
             aqi = excluded.aqi, computed_at = now()",
            &[
                Value::from(d.code.clone()),
                Value::from(d.name.clone()),
                Value::from(d.area_km2),
                Value::from(d.events as i64),
                Value::from(d.bikes as i64),
                Value::from(d.bike_slots as i64),
                Value::from(d.alerts as i64),
                Value::from(d.pm25),
                Value::from(d.aqi),
            ],
        )
        .map_err(|err| err.to_string())?;
    }
    if !output.districts.is_empty() {
        let day = sql::query(
            "select (now() at time zone 'Europe/Helsinki')::date::text",
            &[],
        )
        .ok()
        .and_then(
            |rows| match rows.values.first().and_then(|row| row.first()) {
                Some(Value::Text(day)) => Some(day.clone()),
                _ => None,
            },
        )
        .unwrap_or_default();
        blob::put(
            BOUNDARIES,
            boundaries(&input).to_string().as_bytes(),
            Some("application/geo+json"),
        )
        .map_err(|err| err.to_string())?;
        blob::put(
            LICENCE,
            format!("{LICENCE_TEXT}\nWritten: {day}\n").as_bytes(),
            Some("text/plain; charset=utf-8"),
        )
        .map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// Whether today's figures are kept, on Helsinki's calendar.
fn kept_today() -> Result<bool, sql::Error> {
    let rows = sql::query(
        "select count(*) from district_metrics where day = (now() at time zone 'Europe/Helsinki')::date",
        &[],
    )?;
    Ok(!matches!(
        rows.values.first().and_then(|row| row.first()),
        Some(Value::Int(0)) | None
    ))
}

/// The job `keep-metrics` (app.yaml `spec.server.jobs`): keeps today's figures once, and an hour
/// later again after a run the feeds failed. Reading the five feeds takes longer than the 5 s a
/// request may run, and the job has 60 s (T-6503).
pub fn keep_metrics() -> Result<(), String> {
    if kept_today().map_err(|err| err.to_string())? {
        return Ok(());
    }
    refresh()
}

fn metrics(request: &Request, _: &Params) -> Response {
    let codes = match codes(request.param("codes")) {
        Ok(codes) => codes,
        Err(why) => return Response::problem(400, "Bad Request", &why),
    };
    let today = match kept_today() {
        Ok(today) => today,
        Err(err) => return Response::from_sql(err),
    };
    match sql::query(
        "select day::text as day, code, name, area_km2, events, bikes, bike_slots, alerts, pm25, aqi from district_metrics \
         where code = any(string_to_array($1, ',')) and day > (now() at time zone 'Europe/Helsinki')::date - $2::int \
         order by day desc, code",
        &[Value::from(codes.join(",")), Value::from(DAYS)],
    ) {
        Ok(rows) => Response::json(200, &json!({ "days": sql::objects(&rows), "stale": !today })),
        Err(err) => Response::from_sql(err),
    }
}

fn files(_: &Request, _: &Params) -> Response {
    match blob::list("boundaries/") {
        Ok(keys) if !keys.iter().any(|k| k == BOUNDARIES) => Response::problem(
            404,
            "Not Found",
            "the server has not kept the boundaries yet: they are written with the first day's figures",
        ),
        Ok(_) => match (
            blob::presign(BOUNDARIES, Method::Get, URL_SECONDS),
            blob::presign(LICENCE, Method::Get, URL_SECONDS),
        ) {
            (Ok(geojson), Ok(licence)) => Response::json(200, &json!({ "geojson": geojson, "licence": licence })),
            (Err(err), _) | (_, Err(err)) => Response::from_blob(err),
        },
        Err(err) => Response::from_blob(err),
    }
}

fn bad(why: &str) -> Response {
    Response::problem(400, "Bad Request", why)
}

fn id(params: &Params) -> Result<i64, Response> {
    params["id"]
        .parse()
        .map_err(|_| Response::problem(404, "Not Found", "no such comparison"))
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

/// A comparison's row as the page reads it: its codes as a list.
fn comparison_of(mut row: serde_json::Map<String, Json>) -> Json {
    let codes: Vec<Json> = row
        .get("codes")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .split(',')
        .filter(|c| !c.is_empty())
        .map(|c| json!(c))
        .collect();
    row.insert("codes".into(), Json::Array(codes));
    Json::Object(row)
}

const COLUMNS: &str = "id, name, codes, measure, owner_name as \"ownerName\", \
    to_char(created_at at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') as \"createdAt\"";

/// A share token as `migrations/0003_share_tokens.sql` draws it: 64 lowercase hex characters.
fn is_share(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn comparisons(_: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    match sql::query(
        &format!(
            "select {COLUMNS}, share from comparisons where owner_id = $1 order by id desc limit 50"
        ),
        &[Value::from(caller.id.as_str())],
    ) {
        Ok(rows) => Response::json(
            200,
            &sql::objects(&rows)
                .into_iter()
                .map(comparison_of)
                .collect::<Vec<_>>(),
        ),
        Err(err) => Response::from_sql(err),
    }
}

fn keep(request: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let new: NewComparison = match request.json() {
        Ok(new) => new,
        Err(answer) => return answer,
    };
    let (name, codes) = match checked_comparison(&new) {
        Ok(checked) => checked,
        Err(why) => return bad(&why),
    };
    match sql::query(
        "select count(*) from comparisons where owner_id = $1",
        &[Value::from(caller.id.as_str())],
    ) {
        Ok(rows) => {
            if let Some(Value::Int(n)) = rows.values.first().and_then(|row| row.first()) {
                if *n >= MOST_COMPARISONS {
                    return Response::problem(
                        409,
                        "Conflict",
                        &format!(
                            "at most {MOST_COMPARISONS} comparisons are kept; delete one first"
                        ),
                    );
                }
            }
        }
        Err(err) => return Response::from_sql(err),
    }
    match COMPARISONS.insert(
        &caller,
        &["name", "codes", "measure", "owner_name"],
        vec![
            Value::from(name.as_str()),
            Value::from(codes.as_str()),
            Value::from(new.measure.as_str()),
            Value::from(caller.name.as_str()),
        ],
        &format!("kept the comparison {name}"),
    ) {
        Ok(rows) => {
            let row = sql::objects(&rows).into_iter().next().unwrap_or_default();
            Response::json(
                201,
                &json!({"id": row.get("id"), "share": row.get("share"), "name": name, "codes": new.codes, "measure": new.measure, "ownerName": caller.name}),
            )
        }
        Err(answer) => answer,
    }
}

/// The caller's own comparison by its id; anyone else's is as absent as a missing one.
fn own(_: &Request, params: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let id = match id(params) {
        Ok(id) => id,
        Err(answer) => return answer,
    };
    one(sql::query(
        &format!(
            "select {COLUMNS}, share, true as mine from comparisons where id = $1 and owner_id = $2"
        ),
        &[Value::Int(id), Value::from(caller.id.as_str())],
    ))
}

/// The comparison a shared link names by its token: read-only, its owner named. A token that is
/// not one is answered as a missing one, before the database is asked.
fn shared(_: &Request, params: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let token = &params["share"];
    if !is_share(token) {
        return Response::problem(404, "Not Found", "no such comparison");
    }
    one(sql::query(
        &format!("select {COLUMNS}, owner_id = $2 as mine from comparisons where share = $1"),
        &[Value::from(token.as_str()), Value::from(caller.id.as_str())],
    ))
}

/// The one comparison a read found, or 404.
fn one(read: Result<sql::Rows, sql::Error>) -> Response {
    match read {
        Ok(rows) => match sql::objects(&rows).into_iter().next() {
            Some(row) => Response::json(200, &comparison_of(row)),
            None => Response::problem(404, "Not Found", "no such comparison"),
        },
        Err(err) => Response::from_sql(err),
    }
}

fn change(request: &Request, params: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let id = match id(params) {
        Ok(id) => id,
        Err(answer) => return answer,
    };
    let new: NewComparison = match request.json() {
        Ok(new) => new,
        Err(answer) => return answer,
    };
    let (name, codes) = match checked_comparison(&new) {
        Ok(checked) => checked,
        Err(why) => return bad(&why),
    };
    match COMPARISONS.update(
        &caller,
        id,
        &["name", "codes", "measure"],
        vec![
            Value::from(name.as_str()),
            Value::from(codes.as_str()),
            Value::from(new.measure.as_str()),
        ],
        &format!("changed the comparison {name}"),
    ) {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

fn forget(_: &Request, params: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let id = match id(params) {
        Ok(id) => id,
        Err(answer) => return answer,
    };
    match COMPARISONS.delete(&caller, id, "deleted a comparison") {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

/// A steward, checked before the body is read, so nothing in the body stands in for the role.
fn steward() -> Result<Caller, Response> {
    let caller = Caller::require()?;
    caller.require_role(&[STEWARD])?;
    Ok(caller)
}

/// The newest pin: the comparison everyone opens first.
fn pinned(_: &Request, _: &Params) -> Response {
    match sql::query(
        &format!("select {COLUMNS} from pins order by id desc limit 1"),
        &[],
    ) {
        Ok(rows) => Response::json(
            200,
            &sql::objects(&rows)
                .into_iter()
                .next()
                .map_or(Json::Null, comparison_of),
        ),
        Err(err) => Response::from_sql(err),
    }
}

fn pin(request: &Request, _: &Params) -> Response {
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let new: NewComparison = match request.json() {
        Ok(new) => new,
        Err(answer) => return answer,
    };
    let (name, codes) = match checked_comparison(&new) {
        Ok(checked) => checked,
        Err(why) => return bad(&why),
    };
    match PINS.insert(
        &caller,
        &["name", "codes", "measure", "owner_name"],
        vec![
            Value::from(name.as_str()),
            Value::from(codes.as_str()),
            Value::from(new.measure.as_str()),
            Value::from(caller.name.as_str()),
        ],
        &format!("pinned {name} for everyone"),
    ) {
        Ok(_) => pinned(request, &Params::default()),
        Err(answer) => answer,
    }
}

fn unpin(_: &Request, _: &Params) -> Response {
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let newest = match sql::query("select id from pins order by id desc limit 1", &[]) {
        Ok(rows) => match rows.values.first().and_then(|row| row.first()) {
            Some(Value::Int(id)) => *id,
            _ => return Response::problem(404, "Not Found", "nothing is pinned"),
        },
        Err(err) => return Response::from_sql(err),
    };
    match PINS.delete(&caller, newest, "took back the pinned comparison") {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

/// The newest changes: a steward's every one, anyone else's their own.
fn history(_: &Request, _: &Params) -> Response {
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
        .get("/api/metrics", metrics)
        .get("/api/boundaries", files)
        .get("/api/me", me)
        .get("/api/comparisons", comparisons)
        .post("/api/comparisons", keep)
        .get("/api/comparisons/{id}", own)
        .get("/api/shared/{share}", shared)
        .put("/api/comparisons/{id}", change)
        .delete("/api/comparisons/{id}", forget)
        .get("/api/default", pinned)
        .put("/api/default", pin)
        .delete("/api/default", unpin)
        .get("/api/history", history)
        .handle(&request)
}

jc_app_sdk::app!(handle);

// The job's export, a root-level `keep-metrics: func() -> result<_, string>` beside the request
// handler, as `wit/server.wit` declares it (AP-159).
#[cfg(target_arch = "wasm32")]
mod job {
    wit_bindgen::generate!({ path: "wit", world: "helsinki:district-compare/server", generate_all });

    struct Jobs;

    impl Guest for Jobs {
        fn keep_metrics() -> Result<(), String> {
            super::keep_metrics()
        }
    }

    export!(Jobs);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: [[f64; 2]; 5] = [
        [24.90, 60.15],
        [24.96, 60.15],
        [24.96, 60.18],
        [24.90, 60.18],
        [24.90, 60.15],
    ];

    fn district(code: Json, name: Json, level: &str) -> Json {
        json!({"id": "urn:ngsi-ld:AdministrativeArea:d", "type": "AdministrativeArea", "districtCode": code, "name": name, "divisionLevel": level,
               "location": {"type": "Polygon", "coordinates": [SQUARE]}})
    }

    fn at(lon: f64, lat: f64) -> Json {
        json!({"type": "Point", "coordinates": [lon, lat]})
    }

    #[test]
    fn a_place_is_a_points_coordinates_or_a_shapes_first_vertex() {
        assert_eq!(point_of(Some(&at(24.9, 60.1))), Some(vec![24.9, 60.1]));
        assert_eq!(
            point_of(Some(&json!({"type": "Polygon", "coordinates": [SQUARE]}))),
            Some(vec![24.90, 60.15])
        );
        assert_eq!(
            point_of(Some(&json!({"type": "Point", "coordinates": ["a", 1]}))),
            None
        );
        assert_eq!(point_of(Some(&json!("Mannerheimintie"))), None);
        assert_eq!(point_of(None), None);
    }

    #[test]
    fn a_number_may_be_written_as_text() {
        assert_eq!(number_of(Some(&json!(12))), Some(12.0));
        assert_eq!(number_of(Some(&json!(" 7.5 "))), Some(7.5));
        assert_eq!(number_of(Some(&json!("many"))), None);
        assert_eq!(number_of(None), None);
    }

    #[test]
    fn the_input_is_built_as_the_browser_builds_it() {
        let input = input(
            &[
                district(
                    json!(101),
                    json!({"fi": "Vironniemi", "sv": "Estnäs"}),
                    "district",
                ),
                district(json!("102"), json!("urn:ngsi-ld:x"), "district"),
                district(json!("10"), json!("Eteläinen"), "major"),
            ],
            &[
                json!({"location": at(24.93, 60.16)}),
                json!({"id": "no place"}),
            ],
            &[json!({"location": at(24.93, 60.16), "totalSlotNumber": "20"})],
            &[],
            &[json!({"location": at(24.93, 60.16), "pm25": 4.2, "airQualityIndex": "2"})],
        );
        assert_eq!(
            input
                .districts
                .iter()
                .map(|d| (d.code.as_str(), d.name.as_str()))
                .collect::<Vec<_>>(),
            [("101", "Vironniemi"), ("102", "102")]
        );
        assert_eq!(
            input.events[1],
            vec![999.0, 999.0],
            "no place counts outside every district"
        );
        let output = run(&input);
        let first = &output.districts.iter().find(|d| d.code == "101").unwrap();
        assert_eq!((first.events, first.bikes, first.bike_slots), (1, 1, 20));
        assert_eq!((first.pm25, first.aqi), (Some(4.2), Some(2.0)));
        assert_eq!(output.outside.events, 1);
        let geojson = boundaries(&input);
        assert_eq!(geojson["features"].as_array().map(Vec::len), Some(2));
        assert_eq!(geojson["features"][0]["properties"]["name"], "Vironniemi");
    }

    #[test]
    fn a_request_names_one_to_six_codes() {
        assert_eq!(
            codes(Some("101, 102")),
            Ok(vec!["101".into(), "102".into()])
        );
        assert!(codes(None).is_err());
        assert!(codes(Some(",,")).is_err());
        assert!(codes(Some("1,2,3,4,5,6,7")).is_err());
        assert!(codes(Some("101;drop")).is_err());
        assert!(codes(Some(&"9".repeat(21))).is_err());
    }

    #[test]
    fn the_licence_names_its_source_and_terms() {
        assert!(LICENCE_TEXT.contains("CC BY 4.0") && LICENCE_TEXT.contains("helsingin-piirijako"));
    }

    #[test]
    fn a_route_outside_the_api_or_without_codes_is_answered_in_words() {
        let get = |path: &str| {
            handle(Request {
                method: "GET".into(),
                path: path.into(),
                ..Request::default()
            })
        };
        assert_eq!(get("/api/nothing").status, 404);
        let bare = get("/api/metrics");
        assert_eq!(bare.status, 400);
        assert!(String::from_utf8_lossy(&bare.body).contains("?codes=101,102"));
    }

    fn named(name: &str, codes: &[&str], measure: &str) -> NewComparison {
        NewComparison {
            name: name.into(),
            codes: codes.iter().map(|c| (*c).to_owned()).collect(),
            measure: measure.into(),
        }
    }

    #[test]
    fn a_comparison_is_a_name_up_to_six_districts_and_a_measure() {
        assert_eq!(
            checked_comparison(&named(" Centre ", &["101", "102"], "events")),
            Ok(("Centre".into(), "101,102".into()))
        );
        for bad in [
            named("", &["101"], "events"),
            named(&"x".repeat(81), &["101"], "events"),
            named("two\nlines", &["101"], "events"),
            named("n", &[], "events"),
            named("n", &["1", "2", "3", "4", "5", "6", "7"], "events"),
            named("n", &["101", "101"], "events"),
            named("n", &["101,102"], "events"),
            named("n", &["10 1"], "events"),
            named("n", &["101"], "noise"),
        ] {
            assert!(checked_comparison(&bad).is_err(), "{bad:?}");
        }
        assert!(serde_json::from_str::<NewComparison>(
            r#"{"name":"n","codes":["101"],"measure":"events","owner_id":"u-1"}"#
        )
        .is_err());
    }

    #[test]
    fn a_share_token_is_64_lowercase_hex_characters() {
        assert!(is_share(&"0a".repeat(32)));
        for bad in [
            String::new(),
            "1".into(),
            "0a".repeat(31),
            "0a".repeat(33),
            "0A".repeat(32),
            format!("{}g", "0".repeat(63)),
            format!("{}%", "0".repeat(63)),
        ] {
            assert!(!is_share(&bad), "{bad}");
        }
    }

    #[test]
    fn a_comparison_reads_its_codes_as_a_list() {
        let row = json!({"id": 1, "codes": "101,102"});
        let Json::Object(row) = row else {
            unreachable!()
        };
        assert_eq!(comparison_of(row)["codes"], json!(["101", "102"]));
        assert_eq!(comparison_of(serde_json::Map::new())["codes"], json!([]));
    }
}
