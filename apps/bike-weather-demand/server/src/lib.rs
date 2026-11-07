//! The server half of bike-weather-demand (T-3355): what the browser cannot keep. The broker holds
//! a station's last seven days; the server trains the browser's own model (`../wasm`) on them once
//! a day per station, through the App's own Endpoint with the caller's token, and keeps the model's
//! weather coefficients, spread and hour-of-week profile in its own schema, so a station's model
//! has a history. The data it trained on is a file under the App's prefix. Picking a station and
//! reading the estimate stays in the browser.
//!
//! | Route | What it does |
//! |---|---|
//! | `GET /api/models?station=<urn>` | the station's kept models, newest first; today's trained first when missing |
//! | `GET /api/models/{id}/snapshot` | a URL to download the data a model was trained on |
//!
//! Roles and edits (T-3827, API/07), decided from `identity::caller()` alone: a steward tunes a
//! station's model inputs, and today's model of that station is trained again on them; everyone
//! reads them.
//!
//! | Route | Who | What it does |
//! |---|---|---|
//! | `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
//! | `GET /api/inputs?station=` | anyone | the station's model inputs, the defaults when none are set |
//! | `PUT /api/inputs?station=` `{weatherStation, radiusKm, days, note}` | steward | inputs set; today's model trained again |
//! | `DELETE /api/inputs?station=` | steward | back to the defaults; today's model trained again |
//! | `GET /api/history` | signed in | the newest changes (a viewer: their own) |

use bike_weather_demand::{run_estimate, BikePointInput, Input, WeatherPointInput};
use jc_app_sdk::access;
use jc_app_sdk::blob::{self, Method};
use jc_app_sdk::gateway;
use jc_app_sdk::http::{Params, Request, Response, Router};
use jc_app_sdk::identity::{self, Caller};
use jc_app_sdk::sql::{self, Value};
use serde::Deserialize;
use serde_json::{json, Value as Json};

/// How long a presigned URL lives; the host allows at most 300 seconds.
const URL_SECONDS: u32 = 120;
/// How far the nearest weather station may be, as the browser's own rule, unless a steward set it.
const WEATHER_KM: f64 = 10.0;
/// Days of history a model trains on unless a steward set it: what the Endpoint may read.
const DAYS: i64 = 7;
const STEWARD: &str = "steward";
const HISTORY: i64 = 100;
/// Weather stations read at most.
const MOST_WEATHER: usize = 500;
/// Models answered at most per station.
const MODELS: i64 = 60;

/// A station id as a request may name it: an NGSI-LD URN, no space, comma, slash or `%`.
pub fn station_id(raw: Option<&str>) -> Result<String, String> {
    let id = raw.unwrap_or_default().trim();
    if id.len() < 5
        || id.len() > 300
        || !id.starts_with("urn:")
        || id
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, ',' | '/' | '%' | '?' | '#'))
    {
        return Err(
            "name the station by its id: ?station=urn:ngsi-ld:BikeHireDockingStation:…".into(),
        );
    }
    Ok(id.to_owned())
}

/// What a steward sets for a station's model: a weather station to use instead of the nearest, how
/// far the nearest may be, and how many days of history it trains on.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewInputs {
    pub weather_station: Option<String>,
    pub radius_km: f64,
    pub days: i64,
    #[serde(default)]
    pub note: String,
}

/// A station's inputs as the server trains with them.
#[derive(Debug, PartialEq)]
pub struct Inputs {
    pub weather_station: Option<String>,
    pub radius_km: f64,
    pub days: i64,
}

impl Default for Inputs {
    fn default() -> Self {
        Self {
            weather_station: None,
            radius_km: WEATHER_KM,
            days: DAYS,
        }
    }
}

/// The inputs and the note as they may be kept, or why not.
pub fn checked_inputs(new: &NewInputs) -> Result<(Inputs, String), String> {
    let weather_station = match new.weather_station.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(id) => Some(
            station_id(Some(id))
                .ok()
                .filter(|id| id.starts_with("urn:ngsi-ld:WeatherObserved:"))
                .ok_or("the weather station is a WeatherObserved id, or none for the nearest")?,
        ),
    };
    if !(new.radius_km.is_finite() && (1.0..=25.0).contains(&new.radius_km)) {
        return Err("the radius is 1 to 25 km".into());
    }
    if !(1..=DAYS).contains(&new.days) {
        return Err(format!("a model trains on 1 to {DAYS} days"));
    }
    let note = new.note.trim();
    if note.chars().count() > 200 || note.chars().any(char::is_control) {
        return Err("the note is one line of at most 200 characters".into());
    }
    Ok((
        Inputs {
            weather_station,
            radius_km: new.radius_km,
            days: new.days,
        },
        note.to_owned(),
    ))
}

/// A storage key for a station's training data on a day, from a hash of its id: an id may hold
/// what a key may not (`..`, `:`).
pub fn snapshot_key(station: &str, day: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in station.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("training/{day}/{hash:016x}.json")
}

/// The great-circle distance in kilometres, as the browser's `haversineKm`.
pub fn haversine_km(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    let (d_lat, d_lon) = ((lat2 - lat1).to_radians(), (lon2 - lon1).to_radians());
    let a = (d_lat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (d_lon / 2.0).sin().powi(2);
    6371.0 * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
}

/// A Point's coordinates, else the first vertex of a shape.
pub fn point_of(value: Option<&Json>) -> Option<(f64, f64)> {
    let mut coordinates = value?.get("coordinates")?;
    while let Some(first) = coordinates
        .as_array()
        .and_then(|c| c.first())
        .filter(|c| c.is_array())
    {
        coordinates = first;
    }
    let pair = coordinates.as_array()?;
    Some((pair.first()?.as_f64()?, pair.get(1)?.as_f64()?))
}

/// The weather station nearest to `at` within `km`, as the browser picks it.
pub fn nearest_weather(at: (f64, f64), stations: &[Json], km: f64) -> Option<String> {
    stations
        .iter()
        .filter_map(|s| {
            let (lon, lat) = point_of(s.get("location"))?;
            let distance = haversine_km(at.0, at.1, lon, lat);
            let id = s["id"].as_str()?.to_owned();
            (distance <= km).then_some((distance, id))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

/// A number, or a number written as text, through a keyValues wrapper.
fn number_of(value: &Json) -> Option<f64> {
    match value {
        Json::Number(n) => n.as_f64(),
        Json::String(text) => text.trim().parse().ok().filter(|v: &f64| v.is_finite()),
        Json::Object(o) => o
            .get("@value")
            .or_else(|| o.get("value"))
            .and_then(number_of),
        _ => None,
    }
}

/// One attribute's points of a temporal answer, `(value, observedAt)`, in either of the shapes the
/// broker writes (`temporalValues` pairs, or `{value, observedAt}` instances).
pub fn series(entity: &Json, attr: &str) -> Vec<(f64, String)> {
    let values = match entity.get(attr) {
        Some(Json::Object(o)) => o
            .get("values")
            .and_then(Json::as_array)
            .cloned()
            .unwrap_or_default(),
        Some(Json::Array(list)) => list.clone(),
        _ => Vec::new(),
    };
    values
        .iter()
        .filter_map(|entry| match entry {
            Json::Array(pair) if pair.len() >= 2 => {
                Some((number_of(&pair[0])?, pair[1].as_str()?.to_owned()))
            }
            Json::Object(o) => Some((
                number_of(o.get("value")?)?,
                o.get("observedAt")?.as_str()?.to_owned(),
            )),
            _ => None,
        })
        .collect()
}

/// The weather points of a station's temperature and precipitation, by time, as the browser joins them.
pub fn weather_points(entity: &Json) -> Vec<WeatherPointInput> {
    let mut by_time: std::collections::BTreeMap<String, (Option<f64>, Option<f64>)> =
        Default::default();
    for (value, at) in series(entity, "temperature") {
        by_time.entry(at).or_default().0 = Some(value);
    }
    for (value, at) in series(entity, "precipitation") {
        by_time.entry(at).or_default().1 = Some(value);
    }
    by_time
        .into_iter()
        .map(|(at, (temperature, precipitation))| WeatherPointInput {
            at,
            temperature,
            precipitation,
        })
        .collect()
}

fn read(path: &str) -> Result<Vec<Json>, Response> {
    gateway::get_json::<Vec<Json>>(path).map_err(Response::from)
}

/// The scalar the database answers for `statement`, as text.
fn scalar(statement: &str) -> Result<String, Response> {
    let rows = sql::query(statement, &[]).map_err(Response::from_sql)?;
    match rows.values.first().and_then(|row| row.first()) {
        Some(Value::Text(text)) => Ok(text.clone()),
        Some(Value::Int(n)) => Ok(n.to_string()),
        _ => Err(Response::problem(
            503,
            "Service Unavailable",
            "the database gave no answer",
        )),
    }
}

/// The inputs a steward set for `station`, else the defaults.
fn inputs_of(station: &str) -> Result<Inputs, Response> {
    let rows = sql::query(
        "select weather_station, radius_km, days from station_inputs where station = $1",
        &[Value::from(station)],
    )
    .map_err(Response::from_sql)?;
    Ok(match rows.values.first().map(Vec::as_slice) {
        Some([weather, Value::Float(radius_km), Value::Int(days)]) => Inputs {
            weather_station: match weather {
                Value::Text(id) => Some(id.clone()),
                _ => None,
            },
            radius_km: *radius_km,
            days: *days,
        },
        _ => Inputs::default(),
    })
}

/// Trains today's model of `station` on its last days, as its inputs say, and keeps it with its
/// training data.
fn train(station: &str) -> Result<(), Response> {
    let inputs = inputs_of(station)?;
    let id = gateway::encode(station);
    let found = read(&format!(
        "/ngsi-ld/v1/entities?type=BikeHireDockingStation&id={id}&options=keyValues&attrs=location,totalSlotNumber"
    ))?;
    let Some(entity) = found.iter().find(|e| e["id"].as_str() == Some(station)) else {
        return Err(Response::problem(
            404,
            "Not Found",
            "the Endpoint holds no such station",
        ));
    };
    let total_slots = entity
        .get("totalSlotNumber")
        .and_then(number_of)
        .unwrap_or(0.0)
        .max(0.0) as usize;
    let since = scalar(&format!(
        r#"select to_char((now() - interval '{} days') at time zone 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')"#,
        inputs.days
    ))?;
    let now: i64 = scalar("select extract(epoch from now())::bigint")?
        .parse()
        .unwrap_or_default();
    let day = scalar("select (now() at time zone 'Europe/Helsinki')::date::text")?;
    let history = read(&format!(
        "/ngsi-ld/v1/temporal/entities?type=BikeHireDockingStation&options=temporalValues&timerel=after&timeAt={}&id={id}&attrs=availableBikeNumber",
        gateway::encode(&since)
    ))?;
    let bikes: Vec<BikePointInput> = history
        .iter()
        .find(|e| e["id"].as_str() == Some(station))
        .map(|e| series(e, "availableBikeNumber"))
        .unwrap_or_default()
        .into_iter()
        .map(|(value, at)| BikePointInput { at, value })
        .collect();
    // A weather station that cannot be read leaves the model without its weather terms, as the
    // browser's estimate does. A steward's choice wins over the nearest.
    let weather_station = inputs.weather_station.clone().or_else(|| point_of(entity.get("location")).and_then(|at| {
        let stations = read(&format!(
            "/ngsi-ld/v1/entities?type=WeatherObserved&options=keyValues&attrs=location&limit={MOST_WEATHER}"
        ))
        .ok()?;
        nearest_weather(at, &stations, inputs.radius_km)
    }));
    let weather: Vec<WeatherPointInput> = match &weather_station {
        Some(w) => read(&format!(
            "/ngsi-ld/v1/temporal/entities?type=WeatherObserved&options=temporalValues&timerel=after&timeAt={}&id={}&attrs=temperature,precipitation",
            gateway::encode(&since),
            gateway::encode(w)
        ))
        .ok()
        .and_then(|list| list.into_iter().find(|e| e["id"].as_str() == Some(w.as_str())))
        .map(|e| weather_points(&e))
        .unwrap_or_default(),
        None => Vec::new(),
    };
    let snapshot = json!({
        "station": station, "weatherStation": weather_station, "trainedOn": day, "days": inputs.days, "radiusKm": inputs.radius_km, "since": since, "totalSlots": total_slots,
        "bikes": bikes.iter().map(|b| json!({"at": b.at, "value": b.value})).collect::<Vec<_>>(),
        "weather": weather.iter().map(|w| json!({"at": w.at, "temperature": w.temperature, "precipitation": w.precipitation})).collect::<Vec<_>>(),
    });
    let output = run_estimate(&Input {
        now,
        total_slots,
        bikes,
        weather,
    });
    let key = snapshot_key(station, &day);
    blob::put(
        &key,
        snapshot.to_string().as_bytes(),
        Some("application/json"),
    )
    .map_err(Response::from_blob)?;
    sql::execute(
        "insert into station_models (station, trained_on, hours, enough, per_degree, rain, sigma, weather_station, profile, snapshot, days) \
         values ($1, $2::date, $3, $4, $5::float8, $6::float8, $7::float8, $8::text, $9::jsonb, $10, $11) on conflict (station, trained_on) do nothing",
        &[
            Value::from(station),
            Value::from(day),
            Value::from(output.hours as i64),
            Value::from(output.enough),
            Value::from(output.weather.as_ref().map(|w| w.per_degree)),
            Value::from(output.weather.as_ref().map(|w| w.rain)),
            Value::from(output.sigma),
            Value::from(weather_station),
            Value::Json(serde_json::to_string(&output.profile).unwrap_or_else(|_| "[]".into())),
            Value::from(key),
            Value::from(inputs.days),
        ],
    )
    .map(drop)
    .map_err(Response::from_sql)
}

fn models(request: &Request, _: &Params) -> Response {
    let station = match station_id(request.param("station")) {
        Ok(station) => station,
        Err(why) => return Response::problem(400, "Bad Request", &why),
    };
    let today = match sql::query(
        "select count(*) from station_models where station = $1 and trained_on = (now() at time zone 'Europe/Helsinki')::date",
        &[Value::from(station.as_str())],
    ) {
        Ok(rows) => !matches!(rows.values.first().and_then(|row| row.first()), Some(Value::Int(0)) | None),
        Err(err) => return Response::from_sql(err),
    };
    // A station the Endpoint does not hold is said; data that cannot be read leaves the kept
    // models readable, and says so.
    let problem = if today { None } else { train(&station).err() };
    if problem.as_ref().is_some_and(|answer| answer.status == 404) {
        return problem.unwrap_or_else(Response::no_content);
    }
    match sql::query(
        r#"select id, trained_on::text as trained_on, to_char(trained_at at time zone 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as trained_at,
                  hours, enough, per_degree, rain, sigma, weather_station, days
           from station_models where station = $1 order by trained_on desc limit $2"#,
        &[Value::from(station.as_str()), Value::from(MODELS)],
    ) {
        Ok(rows) if rows.values.is_empty() && problem.is_some() => {
            problem.unwrap_or_else(Response::no_content)
        }
        Ok(rows) => Response::json(
            200,
            &json!({ "models": sql::objects(&rows), "stale": problem.is_some() }),
        ),
        Err(err) => Response::from_sql(err),
    }
}

fn download(_: &Request, params: &Params) -> Response {
    let Ok(id) = params["id"].parse::<i64>() else {
        return Response::problem(404, "Not Found", "no such model");
    };
    match sql::query(
        "select snapshot from station_models where id = $1",
        &[Value::from(id)],
    ) {
        Ok(rows) => match rows.values.first().and_then(|row| row.first()) {
            Some(Value::Text(key)) => match blob::presign(key, Method::Get, URL_SECONDS) {
                Ok(url) => Response::json(200, &json!({ "url": url })),
                Err(err) => Response::from_blob(err),
            },
            _ => Response::problem(404, "Not Found", "no such model"),
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

fn inputs(request: &Request, _: &Params) -> Response {
    let station = match station_id(request.param("station")) {
        Ok(station) => station,
        Err(why) => return Response::problem(400, "Bad Request", &why),
    };
    match sql::query(
        r#"select weather_station as "weatherStation", radius_km as "radiusKm", days, note,
                  updated_by_name as "updatedBy", to_char(updated_at at time zone 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as "updatedAt"
           from station_inputs where station = $1"#,
        &[Value::from(station.as_str())],
    ) {
        Ok(rows) => {
            let kept = sql::objects(&rows).into_iter().next();
            let mut answer = kept.clone().map_or_else(
                || json!({"weatherStation": null, "radiusKm": WEATHER_KM, "days": DAYS, "note": "", "updatedBy": null, "updatedAt": null}),
                Json::Object,
            );
            answer["station"] = json!(station);
            answer["custom"] = json!(kept.is_some());
            Response::json(200, &answer)
        }
        Err(err) => Response::from_sql(err),
    }
}

/// A steward and the station named, checked before the body is read, so nothing in the body
/// stands in for the role.
fn steward(request: &Request) -> Result<(Caller, String), Response> {
    let caller = Caller::require()?;
    caller.require_role(&[STEWARD])?;
    let station = station_id(request.param("station"))
        .map_err(|why| Response::problem(400, "Bad Request", &why))?;
    Ok((caller, station))
}

/// The end of a station's id, for a history line.
fn local(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}

/// Today's model of `station` goes, so the next read trains it again on the inputs now set.
fn retrain(station: &str) -> Result<(), Response> {
    sql::execute(
        "delete from station_models where station = $1 and trained_on = (now() at time zone 'Europe/Helsinki')::date",
        &[Value::from(station)],
    )
    .map(drop)
    .map_err(Response::from_sql)
}

fn set_inputs(request: &Request, params: &Params) -> Response {
    let (caller, station) = match steward(request) {
        Ok(pair) => pair,
        Err(answer) => return answer,
    };
    let new: NewInputs = match request.json() {
        Ok(new) => new,
        Err(answer) => return answer,
    };
    let (set, note) = match checked_inputs(&new) {
        Ok(checked) => checked,
        Err(why) => return Response::problem(400, "Bad Request", &why),
    };
    let saved = sql::execute(
        "insert into station_inputs (station, weather_station, radius_km, days, note, updated_by, updated_by_name) \
         values ($1, $2::text, $3::float8, $4, $5, $6, $7) on conflict (station) do update set \
         weather_station = excluded.weather_station, radius_km = excluded.radius_km, days = excluded.days, \
         note = excluded.note, updated_by = excluded.updated_by, updated_by_name = excluded.updated_by_name, updated_at = now()",
        &[
            Value::from(station.as_str()),
            Value::from(set.weather_station.clone()),
            Value::Float(set.radius_km),
            Value::Int(set.days),
            Value::from(note.as_str()),
            Value::from(caller.id.as_str()),
            Value::from(caller.name.as_str()),
        ],
    )
    .map(drop)
    .map_err(Response::from_sql)
    .and_then(|()| retrain(&station))
    .and_then(|()| {
        let weather = set.weather_station.as_deref().map_or_else(
            || format!("nearest within {} km", set.radius_km),
            |id| local(id).to_owned(),
        );
        access::record(
            &caller,
            "update",
            &format!("inputs/{}", local(&station)),
            &format!("{} days, weather {weather}{}", set.days, if note.is_empty() { String::new() } else { format!(": {note}") }),
        )
    });
    match saved {
        Ok(()) => inputs(request, params),
        Err(answer) => answer,
    }
}

fn reset_inputs(request: &Request, _: &Params) -> Response {
    let (caller, station) = match steward(request) {
        Ok(pair) => pair,
        Err(answer) => return answer,
    };
    let gone = match sql::execute(
        "delete from station_inputs where station = $1",
        &[Value::from(station.as_str())],
    ) {
        Ok(n) => n,
        Err(err) => return Response::from_sql(err),
    };
    if gone == 0 {
        return Response::problem(404, "Not Found", "the station has the defaults already");
    }
    match retrain(&station).and_then(|()| {
        access::record(
            &caller,
            "delete",
            &format!("inputs/{}", local(&station)),
            "back to the defaults",
        )
    }) {
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
        .get("/api/models", models)
        .get("/api/models/{id}/snapshot", download)
        .get("/api/me", me)
        .get("/api/inputs", inputs)
        .put("/api/inputs", set_inputs)
        .delete("/api/inputs", reset_inputs)
        .get("/api/history", history)
        .handle(&request)
}

jc_app_sdk::app!(handle);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_station_is_named_by_a_urn_and_nothing_else() {
        let ok = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001";
        assert_eq!(station_id(Some(&format!(" {ok} "))), Ok(ok.to_owned()));
        for bad in [
            None,
            Some(""),
            Some("001"),
            Some("urn:a b"),
            Some("urn:a,urn:b"),
            Some("urn:../x/y"),
            Some("urn:a%2f"),
        ] {
            assert!(station_id(bad).is_err(), "{bad:?}");
        }
        assert!(station_id(Some(&format!("urn:{}", "x".repeat(300)))).is_err());
    }

    #[test]
    fn a_snapshot_key_is_safe_and_one_per_station_and_day() {
        let key = snapshot_key("urn:ngsi-ld:x:..:y", "2026-10-10");
        let hash = key
            .strip_prefix("training/2026-10-10/")
            .and_then(|k| k.strip_suffix(".json"))
            .unwrap_or_default();
        assert_eq!(hash.len(), 16);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()), "{key}");
        assert_eq!(key, snapshot_key("urn:ngsi-ld:x:..:y", "2026-10-10"));
        assert_ne!(key, snapshot_key("urn:ngsi-ld:x:..:z", "2026-10-10"));
    }

    #[test]
    fn the_nearest_weather_station_within_ten_kilometres_is_picked() {
        let at = |lon: f64, lat: f64, id: &str| json!({"id": id, "location": {"type": "Point", "coordinates": [lon, lat]}});
        let stations = [
            at(24.95, 60.20, "far"),
            at(24.94, 60.17, "near"),
            at(25.50, 60.40, "too far"),
            json!({"id": "nowhere"}),
        ];
        assert_eq!(
            nearest_weather((24.941, 60.171), &stations, WEATHER_KM).as_deref(),
            Some("near")
        );
        assert_eq!(nearest_weather((26.0, 61.0), &stations, WEATHER_KM), None);
        assert_eq!(
            nearest_weather((25.40, 60.37), &stations, 25.0).as_deref(),
            Some("too far")
        );
        assert_eq!(nearest_weather((25.40, 60.37), &stations, 1.0), None);
        assert!((haversine_km(24.94, 60.17, 24.94, 60.27) - 11.12).abs() < 0.05);
    }

    #[test]
    fn a_temporal_answer_is_read_in_both_shapes_and_weather_joined_by_time() {
        let pairs = json!({"id": "w", "temperature": {"type": "Property", "values": [[3.5, "2026-10-08T07:00:00Z"], ["4", "2026-10-08T08:00:00Z"], ["x", "2026-10-08T09:00:00Z"]]},
                           "precipitation": [{"value": 0.2, "observedAt": "2026-10-08T08:00:00Z"}]});
        assert_eq!(
            series(&pairs, "temperature"),
            [
                (3.5, "2026-10-08T07:00:00Z".to_owned()),
                (4.0, "2026-10-08T08:00:00Z".to_owned())
            ]
        );
        let points = weather_points(&pairs);
        assert_eq!(points.len(), 2);
        assert_eq!(
            (points[1].temperature, points[1].precipitation),
            (Some(4.0), Some(0.2))
        );
        assert_eq!(
            (points[0].temperature, points[0].precipitation),
            (Some(3.5), None)
        );
        assert!(series(&json!({}), "temperature").is_empty());
    }

    #[test]
    fn a_route_outside_the_api_or_without_a_station_is_answered_in_words() {
        let get = |path: &str| {
            handle(Request {
                method: "GET".into(),
                path: path.into(),
                ..Request::default()
            })
        };
        assert_eq!(get("/api/nothing").status, 404);
        let bare = get("/api/models");
        assert_eq!(bare.status, 400);
        assert!(String::from_utf8_lossy(&bare.body)
            .contains("?station=urn:ngsi-ld:BikeHireDockingStation"));
        assert_eq!(get("/api/models/x/snapshot").status, 404);
        // The station is named before the database is asked; who may write, the host scenario shows.
        assert_eq!(get("/api/inputs").status, 400);
        assert_eq!(local("urn:ngsi-ld:BikeHireDockingStation:s1"), "s1");
    }

    fn new(weather: Option<&str>, radius_km: f64, days: i64, note: &str) -> NewInputs {
        NewInputs {
            weather_station: weather.map(str::to_owned),
            radius_km,
            days,
            note: note.into(),
        }
    }

    #[test]
    fn inputs_are_a_weather_station_or_the_nearest_a_radius_and_days() {
        let w = "urn:ngsi-ld:WeatherObserved:fmi:kaisaniemi";
        assert_eq!(
            checked_inputs(&new(Some(&format!(" {w} ")), 5.0, 3, " windy ")),
            Ok((
                Inputs {
                    weather_station: Some(w.into()),
                    radius_km: 5.0,
                    days: 3
                },
                "windy".into()
            ))
        );
        assert_eq!(
            checked_inputs(&new(Some(""), 1.0, 7, "")).map(|(i, _)| i.weather_station),
            Ok(None)
        );
        assert_eq!(
            checked_inputs(&new(None, 25.0, 1, "")).map(|(i, _)| i.radius_km),
            Ok(25.0)
        );
        for bad in [
            new(Some("urn:ngsi-ld:BikeHireDockingStation:s1"), 5.0, 3, ""),
            new(Some("kaisaniemi"), 5.0, 3, ""),
            new(None, 0.5, 3, ""),
            new(None, 26.0, 3, ""),
            new(None, f64::NAN, 3, ""),
            new(None, 5.0, 0, ""),
            new(None, 5.0, 8, ""),
            new(None, 5.0, 3, &"x".repeat(201)),
            new(None, 5.0, 3, "two\nlines"),
        ] {
            assert!(checked_inputs(&bad).is_err(), "{bad:?}");
        }
        assert!(serde_json::from_str::<NewInputs>(
            r#"{"weatherStation":null,"radiusKm":5,"days":3,"role":"steward"}"#
        )
        .is_err());
        assert_eq!(Inputs::default().days, 7);
    }
}
