//! The server of air-weather-explorer (T-3348): each station's readings as hourly means, kept in
//! the App's own schema and read again from the App's own Endpoint only for the hours not yet
//! kept, so a visitor no longer pulls a week of every station's raw readings into the browser;
//! comparisons saved under a code a link carries; and CSV exports of the hours compared. The
//! correlations stay in the browser, where every click on the smoothing or the pair recomputes
//! them.
//!
//! | Route | What it does |
//! |---|---|
//! | `GET /api/series?air=&weather=&days=` | both stations' hourly means over the last 1, 3 or 7 days |
//! | `POST /api/comparisons` `{name, station, weather, days, smoothing, air, variable}` | a comparison kept; its code |
//! | `GET /api/comparisons/{code}` | the comparison's choices |
//! | `POST /api/exports` `{air, weather, days}` | the hours as CSV; a URL it downloads from |
//!
//! Roles and edits (T-3827, API/07), decided from `identity::caller()` alone:
//!
//! | Route | Who | What it does |
//! |---|---|---|
//! | `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
//! | `GET /api/places` | signed in | the caller's places, each with its station's latest kept hour |
//! | `POST /api/places` `{station, name, attr, threshold}` | signed in | a place kept |
//! | `PUT /api/places/{id}` `{name, threshold}` | owner | changed |
//! | `DELETE /api/places/{id}` | owner | gone |
//! | `GET /api/faults?station=` | anyone | the periods a steward marked the station's readings faulty |
//! | `POST /api/faults` `{station, from, to, note}` | steward | a period marked |
//! | `DELETE /api/faults/{id}` | steward | the mark taken back |
//! | `GET /api/history` | signed in | the newest changes (a viewer: their own) |

use std::collections::BTreeMap;

use air_weather_explorer::{hourly, Point};
use jc_app_sdk::access::Owned;
use jc_app_sdk::blob::{self, Method};
use jc_app_sdk::gateway;
use jc_app_sdk::http::{Params, Request, Response, Router};
use jc_app_sdk::identity::{self, Caller};
use jc_app_sdk::sql::{self, Value};
use serde::Deserialize;
use serde_json::Value as Json;

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
/// What the App's policies let it read back: a week.
const KEPT: i64 = 7 * DAY;
/// Readings this fresh are not read again.
const FRESH: i64 = 15 * MINUTE;
pub const POLLUTANTS: [&str; 3] = ["pm10", "pm25", "airQualityIndex"];
pub const VARIABLES: [&str; 4] = [
    "temperature",
    "windSpeed",
    "relativeHumidity",
    "precipitation",
];
/// How long an export's download URL lives; the host allows at most 300 seconds.
const URL_SECONDS: u32 = 120;
/// Old comparisons and exports cleared per write, so one write never does unbounded work.
const CLEAR_AT_ONCE: i64 = 20;

/// The App role that marks readings faulty (app.yaml).
const STEWARD: &str = "steward";
/// A place is its keeper's alone: no role changes another's.
const PLACES: Owned = Owned::new("places", "");
const FAULTS: Owned = Owned::new("faults", STEWARD);
/// The most places one person keeps, the longest period one mark covers, and the history read.
const MOST_PLACES: i64 = 20;
const LONGEST_FAULT: i64 = 31 * DAY;
const HISTORY: i64 = 100;

/// A place as the page sends it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewPlace {
    pub station: String,
    pub name: String,
    pub attr: String,
    pub threshold: f64,
}

/// A place's name and level changed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceChange {
    pub name: String,
    pub threshold: f64,
}

/// A period of a station's readings marked faulty, in epoch milliseconds.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewFault {
    pub station: String,
    pub from: i64,
    pub to: i64,
    pub note: String,
}

/// A name or note trimmed, `1..=most` characters, on one line.
pub fn line(text: &str, most: usize, what: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > most || text.chars().any(char::is_control) {
        return Err(format!("a {what} is 1 to {most} characters, on one line"));
    }
    Ok(text.to_owned())
}

/// A level an alert is set at: a number from 0 to 1000.
pub fn threshold(value: f64) -> Result<f64, String> {
    if value.is_finite() && (0.0..=1000.0).contains(&value) {
        Ok(value)
    } else {
        Err("a level is a number from 0 to 1000".into())
    }
}

/// A place kept: an air quality station, its name, one of its pollutants and a level.
pub fn checked_place(new: &NewPlace) -> Result<String, String> {
    station(&new.station, Kind::Air)?;
    if !POLLUTANTS.contains(&new.attr.as_str()) {
        return Err(format!(
            "{} is not a pollutant: pm10, pm25 or airQualityIndex",
            new.attr
        ));
    }
    threshold(new.threshold)?;
    line(&new.name, 80, "name")
}

/// A period marked: its station, a start before its end, at most 31 days, a note on why.
pub fn checked_fault(new: &NewFault) -> Result<String, String> {
    station(&new.station, Kind::Air)?;
    if new.from >= new.to {
        return Err("a period starts before it ends".into());
    }
    if new.to - new.from > LONGEST_FAULT {
        return Err("a period is at most 31 days".into());
    }
    line(&new.note, 200, "note")
}

/// Which of the two kinds of station.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Air,
    Weather,
}

impl Kind {
    pub fn entity_type(self) -> &'static str {
        match self {
            Kind::Air => "AirQualityObserved",
            Kind::Weather => "WeatherObserved",
        }
    }
    pub fn attrs(self) -> &'static [&'static str] {
        match self {
            Kind::Air => &POLLUTANTS,
            Kind::Weather => &VARIABLES,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Kind::Air => "air",
            Kind::Weather => "weather",
        }
    }
}

/// A station's id: a URN of its kind's type, of letters, digits and `._~:-`, at most 256 long.
pub fn station(id: &str, kind: Kind) -> Result<&str, String> {
    let prefix = format!("urn:ngsi-ld:{}:", kind.entity_type());
    let rest = id.strip_prefix(&prefix).unwrap_or_default();
    if rest.is_empty()
        || id.len() > 256
        || !rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._~:-".contains(c))
    {
        return Err(format!(
            "{id} is not the id of an {} station",
            if kind == Kind::Air {
                "air quality"
            } else {
                "weather"
            }
        ));
    }
    Ok(id)
}

/// The period, 1, 3 or 7 days.
pub fn days(text: Option<&str>) -> Result<i64, String> {
    match text.unwrap_or("3") {
        "1" => Ok(1),
        "3" => Ok(3),
        "7" => Ok(7),
        other => Err(format!("{other} is not a period: 1, 3 or 7 days")),
    }
}

/// The civil date of days since 1970-01-01 (Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `2026-10-10T08:00:00Z` of epoch milliseconds, to the second.
pub fn iso(ms: i64) -> String {
    let (y, m, d) = civil_from_days(ms.div_euclid(DAY));
    let s = ms.rem_euclid(DAY) / 1000;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s % 3600 / 60,
        s % 60
    )
}

/// Epoch milliseconds of an RFC 3339 instant, any offset, fractions allowed.
pub fn instant(text: &str) -> Option<i64> {
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let (clock, offset) = if let Some(clock) = time.strip_suffix('Z') {
        (clock, 0)
    } else {
        let at = time.rfind(['+', '-'])?;
        let (clock, zone) = time.split_at(at);
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let (h, mi) = zone[1..].split_once(':')?;
        (
            clock,
            sign * (h.parse::<i64>().ok()? * HOUR + mi.parse::<i64>().ok()? * MINUTE),
        )
    };
    let mut c = clock.split(':');
    let (h, mi): (i64, i64) = (c.next()?.parse().ok()?, c.next()?.parse().ok()?);
    let seconds: f64 = c.next().unwrap_or("0").parse().ok()?;
    if !(1..=12).contains(&m)
        || !(1..=31).contains(&day)
        || h > 23
        || mi > 59
        || !(0.0..61.0).contains(&seconds)
    {
        return None;
    }
    Some(
        days_from_civil(y, m, day) * DAY + h * HOUR + mi * MINUTE + (seconds * 1000.0) as i64
            - offset,
    )
}

/// One station's readings from a temporal answer, attribute by attribute, as the page reads them
/// (src/stations.ts `readingsOf`): numbers only, humidity from a share to per cent.
pub fn readings(entity: &Json, kind: Kind) -> BTreeMap<String, Vec<Point>> {
    let mut out = BTreeMap::new();
    for attr in kind.attrs() {
        let Some(property) = entity.get(*attr) else {
            continue;
        };
        let values = property
            .get("values")
            .and_then(Json::as_array)
            .or_else(|| property.as_array());
        let mut points = Vec::new();
        for entry in values.into_iter().flatten() {
            let (value, at) = match entry {
                Json::Array(pair) if pair.len() >= 2 => {
                    (pair[0].as_f64(), pair[1].as_str().and_then(instant))
                }
                Json::Object(o) => (
                    o.get("value").and_then(Json::as_f64),
                    o.get("observedAt").and_then(Json::as_str).and_then(instant),
                ),
                _ => (None, None),
            };
            if let (Some(value), Some(at)) = (value, at) {
                if value.is_finite() {
                    points.push((
                        at,
                        if *attr == "relativeHumidity" {
                            value * 100.0
                        } else {
                            value
                        },
                    ));
                }
            }
        }
        if !points.is_empty() {
            out.insert((*attr).to_owned(), points);
        }
    }
    out
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn json<T: serde::Serialize>(value: &T) -> Value {
    Value::Json(serde_json::to_string(value).unwrap_or_else(|_| "null".into()))
}

/// Brings the station's hourly means up to now: reads from the App's own Endpoint only from the
/// start of the last hour it kept (or a week back), averages per hour, keeps them, and drops what
/// is older than the period the App may read. `false` when the Endpoint knows no such station.
fn refresh(id: &str, kind: Kind, now: i64) -> Result<bool, Response> {
    let rows = sql::query(
        "select fetched_until from stations where id = $1",
        &[Value::from(id)],
    )
    .map_err(Response::from_sql)?;
    let until = match rows.values.first().and_then(|r| r.first()) {
        Some(Value::Int(until)) => Some(*until),
        _ => None,
    };
    if until.is_some_and(|until| now - until < FRESH) {
        return Ok(true);
    }
    // A minute inside the week the policy allows, and the last kept hour read again whole.
    let floor = now - KEPT + MINUTE;
    let since = until.map_or(floor, |until| {
        (until.div_euclid(HOUR) * HOUR - HOUR).max(floor)
    });
    let path = format!(
        "/ngsi-ld/v1/temporal/entities?type={}&id={}&attrs={}&timerel=after&timeAt={}&options=temporalValues",
        kind.entity_type(),
        gateway::encode(id),
        gateway::encode(&kind.attrs().join(",")),
        gateway::encode(&iso(since)),
    );
    let entities: Vec<Json> = gateway::get_json(&path).map_err(Response::from)?;
    let Some(entity) = entities
        .iter()
        .find(|e| e.get("id").and_then(Json::as_str) == Some(id))
    else {
        return Ok(until.is_some());
    };
    let first_hour = since.div_euclid(HOUR) * HOUR;
    let mut rows = Vec::new();
    for (attr, points) in readings(entity, kind) {
        for (hour, value) in hourly(&points) {
            if hour >= first_hour {
                rows.push(serde_json::json!({"attr": attr, "hour": hour, "value": value}));
            }
        }
    }
    sql::execute(
        "insert into stations (id, kind, fetched_until) values ($1, $2, $3) \
         on conflict (id) do update set fetched_until = excluded.fetched_until",
        &[Value::from(id), Value::from(kind.name()), Value::from(now)],
    )
    .map_err(Response::from_sql)?;
    if !rows.is_empty() {
        sql::execute(
            "insert into hourly (station, attr, hour, value) \
             select $1, r.attr, r.hour, r.value from jsonb_to_recordset($2) as r(attr text, hour bigint, value double precision) \
             on conflict (station, attr, hour) do update set value = excluded.value",
            &[Value::from(id), json(&rows)],
        )
        .map_err(Response::from_sql)?;
    }
    sql::execute(
        "delete from hourly where station = $1 and hour < $2",
        &[Value::from(id), Value::from(now - KEPT - DAY)],
    )
    .map_err(Response::from_sql)?;
    Ok(true)
}

/// Hourly means, attribute by attribute, `(hour, value)`.
type Hours = BTreeMap<String, Vec<(i64, f64)>>;

/// The station's kept hourly means from `from` on.
fn kept(id: &str, from: i64) -> Result<Hours, Response> {
    let rows = sql::query(
        "select attr, hour, value from hourly where station = $1 and hour >= $2 order by attr, hour",
        &[Value::from(id), Value::from(from)],
    )
    .map_err(Response::from_sql)?;
    let mut out = Hours::new();
    for row in &rows.values {
        if let [Value::Text(attr), Value::Int(hour), Value::Float(value)] = row.as_slice() {
            out.entry(attr.clone()).or_default().push((*hour, *value));
        }
    }
    Ok(out)
}

/// Both stations brought up to now, and their hours of the period.
fn both(air: &str, weather: &str, days: i64) -> Result<(Hours, Hours), Response> {
    let now = now();
    if !refresh(air, Kind::Air, now)? {
        return Err(Response::problem(
            404,
            "Not Found",
            "no such air quality station",
        ));
    }
    if !refresh(weather, Kind::Weather, now)? {
        return Err(Response::problem(
            404,
            "Not Found",
            "no such weather station",
        ));
    }
    let from = (now - days * DAY).div_euclid(HOUR) * HOUR;
    Ok((kept(air, from)?, kept(weather, from)?))
}

fn bad(why: &str) -> Response {
    Response::problem(400, "Bad Request", why)
}

fn id(params: &Params, what: &str) -> Result<i64, Response> {
    params["id"]
        .parse()
        .map_err(|_| Response::problem(404, "Not Found", &format!("no such {what}")))
}

fn me(_: &Request, _: &Params) -> Response {
    match identity::caller() {
        Some(caller) => Response::json(
            200,
            &serde_json::json!({"id": caller.id, "name": caller.name, "roles": caller.roles}),
        ),
        None => Response::json(200, &Json::Null),
    }
}

/// The caller's places, newest first, each with the latest hour kept for its station and pollutant.
fn places(_: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    match sql::query(
        "select p.id, p.station, p.name, p.attr, p.threshold, h.value as latest, \
         h.hour as \"latestHour\" from places p left join lateral (select value, hour from hourly \
         where station = p.station and attr = p.attr order by hour desc limit 1) h on true \
         where p.owner_id = $1 order by p.id desc limit 50",
        &[Value::from(caller.id.as_str())],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

fn keep_place(request: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let new: NewPlace = match request.json() {
        Ok(new) => new,
        Err(answer) => return answer,
    };
    let name = match checked_place(&new) {
        Ok(name) => name,
        Err(why) => return bad(&why),
    };
    match sql::query(
        "select count(*) as n from places where owner_id = $1",
        &[Value::from(caller.id.as_str())],
    ) {
        Ok(rows) => {
            let n = sql::objects(&rows)
                .first()
                .and_then(|row| row.get("n").and_then(Json::as_i64))
                .unwrap_or(0);
            if n >= MOST_PLACES {
                return Response::problem(
                    409,
                    "Conflict",
                    &format!("at most {MOST_PLACES} places are kept; delete one first"),
                );
            }
        }
        Err(err) => return Response::from_sql(err),
    }
    match PLACES.insert(
        &caller,
        &["station", "name", "attr", "threshold"],
        vec![
            Value::from(new.station.as_str()),
            Value::from(name.as_str()),
            Value::from(new.attr.as_str()),
            Value::Float(new.threshold),
        ],
        &format!("kept the place {name}"),
    ) {
        Ok(rows) => {
            let row = sql::objects(&rows).into_iter().next().unwrap_or_default();
            Response::json(
                201,
                &serde_json::json!({"id": row.get("id"), "station": new.station, "name": name, "attr": new.attr, "threshold": new.threshold, "latest": null, "latestHour": null}),
            )
        }
        Err(answer) => answer,
    }
}

fn change_place(request: &Request, params: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let id = match id(params, "place") {
        Ok(id) => id,
        Err(answer) => return answer,
    };
    let wanted: PlaceChange = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let (name, level) = match (line(&wanted.name, 80, "name"), threshold(wanted.threshold)) {
        (Ok(name), Ok(level)) => (name, level),
        (Err(why), _) | (_, Err(why)) => return bad(&why),
    };
    match PLACES.update(
        &caller,
        id,
        &["name", "threshold"],
        vec![Value::from(name.as_str()), Value::Float(level)],
        &format!("changed the place {name}"),
    ) {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

fn forget_place(_: &Request, params: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let id = match id(params, "place") {
        Ok(id) => id,
        Err(answer) => return answer,
    };
    match PLACES.delete(&caller, id, "deleted a place") {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

fn faults(request: &Request, _: &Params) -> Response {
    let station = match station(request.param("station").unwrap_or_default(), Kind::Air) {
        Ok(station) => station,
        Err(why) => return bad(&why),
    };
    match sql::query(
        "select id, station, from_ms as \"from\", to_ms as \"to\", note from faults \
         where station = $1 order by from_ms desc limit 50",
        &[Value::from(station)],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

/// A steward, checked before the body is read, so nothing in the body stands in for the role.
fn steward() -> Result<Caller, Response> {
    let caller = Caller::require()?;
    caller.require_role(&[STEWARD])?;
    Ok(caller)
}

fn mark_fault(request: &Request, _: &Params) -> Response {
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let new: NewFault = match request.json() {
        Ok(new) => new,
        Err(answer) => return answer,
    };
    let note = match checked_fault(&new) {
        Ok(note) => note,
        Err(why) => return bad(&why),
    };
    match FAULTS.insert(
        &caller,
        &["station", "from_ms", "to_ms", "note"],
        vec![
            Value::from(new.station.as_str()),
            Value::Int(new.from),
            Value::Int(new.to),
            Value::from(note.as_str()),
        ],
        &format!("marked {} faulty: {note}", local(&new.station)),
    ) {
        Ok(rows) => {
            let row = sql::objects(&rows).into_iter().next().unwrap_or_default();
            Response::json(
                201,
                &serde_json::json!({"id": row.get("id"), "station": new.station, "from": new.from, "to": new.to, "note": note}),
            )
        }
        Err(answer) => answer,
    }
}

fn unmark_fault(_: &Request, params: &Params) -> Response {
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let id = match id(params, "mark") {
        Ok(id) => id,
        Err(answer) => return answer,
    };
    match FAULTS.delete(&caller, id, "took back a faulty mark") {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

/// The end of a station's id, for a history line.
fn local(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
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

fn series(request: &Request, _: &Params) -> Response {
    let (air, weather) = match (
        station(request.param("air").unwrap_or_default(), Kind::Air),
        station(request.param("weather").unwrap_or_default(), Kind::Weather),
    ) {
        (Ok(a), Ok(w)) => (a, w),
        (Err(why), _) | (_, Err(why)) => return bad(&why),
    };
    let days = match days(request.param("days")) {
        Ok(days) => days,
        Err(why) => return bad(&why),
    };
    match both(air, weather, days) {
        Ok((a, w)) => Response::json(200, &serde_json::json!({"air": a, "weather": w})),
        Err(answer) => answer,
    }
}

/// The hours of both stations as CSV: one row per hour either has, a column per attribute, empty
/// where an hour has no reading.
pub fn csv(air_id: &str, weather_id: &str, air: &Hours, weather: &Hours) -> String {
    let mut hours: BTreeMap<i64, Vec<Option<f64>>> = BTreeMap::new();
    let columns: Vec<(&str, &[(i64, f64)])> = POLLUTANTS
        .iter()
        .map(|a| (*a, air.get(*a).map_or(&[][..], Vec::as_slice)))
        .chain(
            VARIABLES
                .iter()
                .map(|v| (*v, weather.get(*v).map_or(&[][..], Vec::as_slice))),
        )
        .collect();
    for (n, (_, points)) in columns.iter().enumerate() {
        for (hour, value) in points.iter() {
            hours
                .entry(*hour)
                .or_insert_with(|| vec![None; columns.len()])[n] = Some(*value);
        }
    }
    let mut out = format!("# air quality station: {air_id}\n# weather station: {weather_id}\n# hourly means; relativeHumidity in per cent\nhour_utc");
    for (name, _) in &columns {
        out.push(',');
        out.push_str(name);
    }
    out.push('\n');
    for (hour, values) in hours {
        out.push_str(&iso(hour));
        for value in values {
            out.push(',');
            if let Some(v) = value {
                out.push_str(&format!("{}", (v * 1000.0).round() / 1000.0));
            }
        }
        out.push('\n');
    }
    out
}

/// A code of 12 letters and digits of a random UUID Postgres makes.
fn new_code() -> Result<String, Response> {
    let rows = sql::query(
        "select substr(replace(gen_random_uuid()::text, '-', ''), 1, 12) as code",
        &[],
    )
    .map_err(Response::from_sql)?;
    match rows.values.first().and_then(|r| r.first()) {
        Some(Value::Text(code)) => Ok(code.clone()),
        _ => Err(Response::problem(
            500,
            "Internal Server Error",
            "no code was made",
        )),
    }
}

pub fn is_code(code: &str) -> bool {
    code.len() == 12
        && code
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Export {
    pub air: String,
    pub weather: String,
    #[serde(default)]
    pub days: Option<u8>,
}

fn export(request: &Request, _: &Params) -> Response {
    let wanted: Export = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let (air, weather) = match (
        station(&wanted.air, Kind::Air),
        station(&wanted.weather, Kind::Weather),
    ) {
        (Ok(a), Ok(w)) => (a, w),
        (Err(why), _) | (_, Err(why)) => return bad(&why),
    };
    let days = match days(wanted.days.map(|d| d.to_string()).as_deref()) {
        Ok(days) => days,
        Err(why) => return bad(&why),
    };
    let (a, w) = match both(air, weather, days) {
        Ok(both) => both,
        Err(answer) => return answer,
    };
    // Exports a day old go first, files before rows.
    if let Ok(old) = sql::query("select code from exports where created_at < now() - interval '1 day' order by created_at limit $1", &[Value::from(CLEAR_AT_ONCE)]) {
        for row in &old.values {
            if let Some(Value::Text(code)) = row.first() {
                if matches!(blob::delete(&format!("exports/{code}.csv")), Ok(()) | Err(blob::Error::NotFound)) {
                    let _ = sql::execute("delete from exports where code = $1", &[Value::from(code.as_str())]);
                }
            }
        }
    }
    let code = match new_code() {
        Ok(code) => code,
        Err(answer) => return answer,
    };
    let key = format!("exports/{code}.csv");
    if let Err(err) = blob::put(
        &key,
        csv(air, weather, &a, &w).as_bytes(),
        Some("text/csv; charset=utf-8"),
    ) {
        return Response::from_blob(err);
    }
    if let Err(err) = sql::execute(
        "insert into exports (code) values ($1)",
        &[Value::from(code.as_str())],
    ) {
        let _ = blob::delete(&key);
        return Response::from_sql(err);
    }
    match blob::presign(&key, Method::Get, URL_SECONDS) {
        Ok(url) => Response::json(
            201,
            &serde_json::json!({"url": url, "expiresIn": URL_SECONDS}),
        ),
        Err(err) => Response::from_blob(err),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comparison {
    #[serde(default)]
    pub name: Option<String>,
    pub station: String,
    pub weather: String,
    pub days: u8,
    pub smoothing: u8,
    #[serde(default)]
    pub air: Option<String>,
    #[serde(default)]
    pub variable: Option<String>,
}

/// A comparison as kept: its stations of the right kinds, a period and smoothing the page offers,
/// a pollutant and a variable it knows, a name of one short line.
pub fn comparison(c: &Comparison) -> Result<Option<String>, String> {
    station(&c.station, Kind::Air)?;
    station(&c.weather, Kind::Weather)?;
    days(Some(&c.days.to_string()))?;
    if !(1..=12).contains(&c.smoothing) {
        return Err("the smoothing is 1 to 12 hours".into());
    }
    if c.air.as_deref().is_some_and(|a| !POLLUTANTS.contains(&a)) {
        return Err("the pollutant is pm10, pm25 or airQualityIndex".into());
    }
    if c.variable
        .as_deref()
        .is_some_and(|v| !VARIABLES.contains(&v))
    {
        return Err(
            "the weather variable is temperature, windSpeed, relativeHumidity or precipitation"
                .into(),
        );
    }
    match c.name.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(name) if name.chars().count() > 80 || name.chars().any(char::is_control) => {
            Err("a name is one line of at most 80 characters".into())
        }
        Some(name) => Ok(Some(name.to_owned())),
    }
}

fn save(request: &Request, _: &Params) -> Response {
    let wanted: Comparison = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let name = match comparison(&wanted) {
        Ok(name) => name,
        Err(why) => return bad(&why),
    };
    // Comparisons half a year old are cleared, a few per save.
    let _ = sql::execute(
        "delete from comparisons where code in (select code from comparisons where created_at < now() - interval '180 days' order by created_at limit $1)",
        &[Value::from(CLEAR_AT_ONCE)],
    );
    let code = match new_code() {
        Ok(code) => code,
        Err(answer) => return answer,
    };
    let saved = sql::execute(
        "insert into comparisons (code, name, station, weather, days, smoothing, air, variable) values ($1, $2, $3, $4, $5, $6, $7, $8)",
        &[
            Value::from(code.as_str()),
            Value::from(name.clone()),
            Value::from(wanted.station.as_str()),
            Value::from(wanted.weather.as_str()),
            Value::from(i64::from(wanted.days)),
            Value::from(i64::from(wanted.smoothing)),
            Value::from(wanted.air.clone()),
            Value::from(wanted.variable.clone()),
        ],
    );
    match saved {
        Ok(_) => Response::json(201, &serde_json::json!({"code": code, "name": name})),
        Err(err) => Response::from_sql(err),
    }
}

fn open(_: &Request, params: &Params) -> Response {
    let code = params["code"].as_str();
    if !is_code(code) {
        return Response::problem(404, "Not Found", "no comparison is saved under this link");
    }
    let rows = sql::query(
        "select code, name, station, weather, days, smoothing, air, variable, created_at as \"createdAt\" from comparisons where code = $1",
        &[Value::from(code)],
    );
    match rows.map(|rows| sql::objects(&rows).into_iter().next()) {
        Ok(Some(c)) => Response::json(200, &c),
        Ok(None) => Response::problem(404, "Not Found", "no comparison is saved under this link"),
        Err(err) => Response::from_sql(err),
    }
}

pub fn handle(request: Request) -> Response {
    Router::new()
        .get("/api/series", series)
        .post("/api/comparisons", save)
        .get("/api/comparisons/{code}", open)
        .post("/api/exports", export)
        .get("/api/me", me)
        .get("/api/places", places)
        .post("/api/places", keep_place)
        .put("/api/places/{id}", change_place)
        .delete("/api/places/{id}", forget_place)
        .get("/api/faults", faults)
        .post("/api/faults", mark_fault)
        .delete("/api/faults/{id}", unmark_fault)
        .get("/api/history", history)
        .handle(&request)
}

jc_app_sdk::app!(handle);

#[cfg(test)]
mod tests {
    use super::*;

    const KALLIO: &str = "urn:ngsi-ld:AirQualityObserved:kallio-2";

    #[test]
    fn a_place_is_an_air_station_a_pollutant_a_level_and_a_name() {
        let place = |station: &str, name: &str, attr: &str, threshold: f64| NewPlace {
            station: station.into(),
            name: name.into(),
            attr: attr.into(),
            threshold,
        };
        assert_eq!(
            checked_place(&place(KALLIO, "  Home ", "pm25", 25.0)),
            Ok("Home".into())
        );
        for (wrong, why) in [
            (
                place("urn:ngsi-ld:WeatherObserved:x", "Home", "pm25", 25.0),
                "air quality",
            ),
            (place(KALLIO, "Home", "temperature", 25.0), "pollutant"),
            (place(KALLIO, "Home", "pm25", -1.0), "level"),
            (place(KALLIO, "Home", "pm25", f64::NAN), "level"),
            (place(KALLIO, "Home", "pm25", 1000.5), "level"),
            (place(KALLIO, " ", "pm25", 25.0), "name"),
            (place(KALLIO, &"x".repeat(81), "pm25", 25.0), "name"),
        ] {
            let err = checked_place(&wrong).expect_err(&wrong.name);
            assert!(err.contains(why), "{err}");
        }
        assert!(serde_json::from_value::<NewPlace>(
            serde_json::json!({"station": KALLIO, "name": "x", "attr": "pm25", "threshold": 1, "owner_id": "u-2"})
        )
        .is_err(), "no owner is taken from the body");
    }

    #[test]
    fn a_fault_is_a_period_of_at_most_a_month_with_a_reason() {
        let fault = |from: i64, to: i64, note: &str| NewFault {
            station: KALLIO.into(),
            from,
            to,
            note: note.into(),
        };
        assert_eq!(
            checked_fault(&fault(0, HOUR, " sensor iced ")),
            Ok("sensor iced".into())
        );
        assert_eq!(
            checked_fault(&fault(0, 31 * DAY, "long")),
            Ok("long".into())
        );
        for (wrong, why) in [
            (fault(HOUR, HOUR, "x"), "starts before"),
            (fault(HOUR, 0, "x"), "starts before"),
            (fault(0, 31 * DAY + 1, "x"), "31 days"),
            (fault(0, HOUR, ""), "note"),
            (fault(0, HOUR, &"x".repeat(201)), "note"),
            (fault(0, HOUR, "a\nb"), "note"),
        ] {
            let err = checked_fault(&wrong).expect_err(&wrong.note);
            assert!(err.contains(why), "{err}");
        }
        assert_eq!(local(KALLIO), "kallio-2");
    }
    use serde_json::json;

    const AIR: &str = "urn:ngsi-ld:AirQualityObserved:hsy:kallio-2";
    const WEATHER: &str = "urn:ngsi-ld:WeatherObserved:fmi:kaisaniemi";

    #[test]
    fn a_station_is_a_urn_of_its_kind() {
        assert_eq!(station(AIR, Kind::Air), Ok(AIR));
        assert!(station(AIR, Kind::Weather).is_err());
        for wrong in [
            "",
            "urn:ngsi-ld:AirQualityObserved:",
            "urn:ngsi-ld:AirQualityObserved:a,b",
            "urn:ngsi-ld:AirQualityObserved:a&type=X",
            "urn:ngsi-ld:AirQualityObserved:a b",
        ] {
            assert!(station(wrong, Kind::Air).is_err(), "{wrong}");
        }
        assert!(station(
            &format!("urn:ngsi-ld:AirQualityObserved:{}", "a".repeat(240)),
            Kind::Air
        )
        .is_err());
    }

    #[test]
    fn the_period_is_one_three_or_seven_days() {
        assert_eq!(days(None), Ok(3));
        assert_eq!(days(Some("7")), Ok(7));
        for wrong in ["0", "2", "30", "-1", "3.0", ""] {
            assert!(days(Some(wrong)).is_err(), "{wrong}");
        }
    }

    #[test]
    fn an_instant_reads_and_writes_in_utc() {
        let ms = days_from_civil(2026, 10, 10) * DAY + 8 * HOUR;
        assert_eq!(iso(ms), "2026-10-10T08:00:00Z");
        assert_eq!(instant("2026-10-10T08:00:00Z"), Some(ms));
        assert_eq!(instant("2026-10-10T11:00:00.000+03:00"), Some(ms));
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(instant("2026-10-10"), None);
        assert_eq!(instant("2026-13-10T08:00:00Z"), None);
    }

    #[test]
    fn readings_are_the_numbers_of_each_attribute_humidity_in_per_cent() {
        let entity = json!({"id": WEATHER, "type": "WeatherObserved",
            "temperature": {"type": "Property", "values": [[4.5, "2026-10-10T08:10:00Z"], ["warm", "2026-10-10T08:20:00Z"], [5.5, "not a time"]]},
            "relativeHumidity": {"type": "Property", "values": [[0.8, "2026-10-10T08:00:00Z"]]},
            "windSpeed": [{"value": 3.0, "observedAt": "2026-10-10T08:00:00Z"}],
            "precipitation": {"type": "Property", "values": []},
            "name": {"type": "Property", "values": [["Kaisaniemi", "2026-10-10T08:00:00Z"]]}});
        let got = readings(&entity, Kind::Weather);
        let at = days_from_civil(2026, 10, 10) * DAY + 8 * HOUR;
        assert_eq!(got.get("temperature"), Some(&vec![(at + 10 * MINUTE, 4.5)]));
        assert_eq!(
            got.get("relativeHumidity").map(|p| p[0].1.round()),
            Some(80.0)
        );
        assert_eq!(got.get("windSpeed"), Some(&vec![(at, 3.0)]));
        assert!(!got.contains_key("precipitation"), "no reading, no series");
        assert!(!got.contains_key("name"), "only the kind's attributes");
        assert!(readings(&json!({"id": AIR}), Kind::Air).is_empty());
    }

    #[test]
    fn the_csv_has_an_hour_per_row_and_a_column_per_attribute() {
        let h = days_from_civil(2026, 10, 10) * DAY;
        let air = BTreeMap::from([("pm25".to_owned(), vec![(h, 4.25), (h + HOUR, 5.0)])]);
        let weather = BTreeMap::from([("windSpeed".to_owned(), vec![(h + HOUR, 2.123_456)])]);
        let text = csv(AIR, WEATHER, &air, &weather);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], format!("# air quality station: {AIR}"));
        assert_eq!(lines[3], "hour_utc,pm10,pm25,airQualityIndex,temperature,windSpeed,relativeHumidity,precipitation");
        assert_eq!(lines[4], "2026-10-10T00:00:00Z,,4.25,,,,,");
        assert_eq!(lines[5], "2026-10-10T01:00:00Z,,5,,,2.123,,");
        assert_eq!(lines.len(), 6);
        assert_eq!(
            csv(AIR, WEATHER, &BTreeMap::new(), &BTreeMap::new())
                .lines()
                .count(),
            4
        );
    }

    #[test]
    fn a_comparison_names_what_the_page_offers() {
        let ok = Comparison {
            name: Some("  Kallio and wind ".into()),
            station: AIR.into(),
            weather: WEATHER.into(),
            days: 3,
            smoothing: 3,
            air: Some("pm25".into()),
            variable: Some("windSpeed".into()),
        };
        assert_eq!(comparison(&ok), Ok(Some("Kallio and wind".into())));
        let with = |edit: fn(&mut Comparison)| {
            let mut c = Comparison {
                name: None,
                station: AIR.into(),
                weather: WEATHER.into(),
                days: 3,
                smoothing: 3,
                air: None,
                variable: None,
            };
            edit(&mut c);
            comparison(&c)
        };
        assert_eq!(with(|_| {}), Ok(None));
        assert!(with(|c| c.days = 2).is_err());
        assert!(with(|c| c.smoothing = 0).is_err());
        assert!(with(|c| c.smoothing = 13).is_err());
        assert!(with(|c| c.air = Some("no2".into())).is_err());
        assert!(with(|c| c.variable = Some("pressure".into())).is_err());
        assert!(with(|c| c.station = WEATHER.into()).is_err());
        assert!(with(|c| c.name = Some("x".repeat(81))).is_err());
        assert!(with(|c| c.name = Some("two\nlines".into())).is_err());
    }

    #[test]
    fn a_code_is_twelve_lowercase_letters_or_digits() {
        assert!(is_code("abc123def456"));
        for wrong in ["", "ABC123DEF456", "abc", "abc123def45/"] {
            assert!(!is_code(wrong), "{wrong}");
        }
    }
}
