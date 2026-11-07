//! The server of bike-rebalancing (T-3346): saved plans, the routes a van actually drove, and a
//! route sheet per plan for the driver. The page still plans in the browser while the operator
//! clicks; a saved plan is planned again here from the counts the gateway gives at that moment,
//! with the caller's own rights, so what is kept is what the city's data said and not what a
//! browser sent.
//!
//! Roles (T-3826, API/07), from `identity::caller()` alone: a viewer saves plans, records drives
//! and reads the history of their own; a steward reads and changes every plan, hands a plan to
//! another van or crew, sets the planner's levels for everyone and reads the whole history. A plan
//! that is not the caller's answers `404`, as if it did not exist. Every write leaves a row in
//! `app_audit` in the same statement.
//!
//! | Route | Who | What it does |
//! |---|---|---|
//! | `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
//! | `GET /api/levels` | anyone | the planner's levels `{low, high, target}` |
//! | `PUT /api/levels` `{low, high, target}` | steward | the levels changed for every plan after |
//! | `GET /api/plans?operator=` | signed in | the latest plans (a steward: everyone's), of one operator when named |
//! | `POST /api/plans` `{operator, vanCapacity, start, include, exclude}` | signed in | a plan made and kept |
//! | `GET /api/plans/{id}` | owner, steward | the plan, its stops and its drives |
//! | `DELETE /api/plans/{id}` | owner, steward | the plan, its drives and its sheet gone |
//! | `PUT /api/plans/{id}/operator` `{operator}` | steward | the plan handed to another van or crew |
//! | `POST /api/plans/{id}/drives` `{stops, km, note}` | owner, steward | a drive of the plan recorded |
//! | `GET /api/plans/{id}/sheet` | owner, steward | a URL the route sheet (CSV) downloads from |
//! | `GET /api/history` | signed in | the newest changes (a viewer: their own) |

use bike_rebalancing::{run, Input, Settings, Station, Stop};
use jc_app_sdk::access::Owned;
use jc_app_sdk::blob::{self, Method};
use jc_app_sdk::gateway;
use jc_app_sdk::http::{Params, Request, Response, Router};
use jc_app_sdk::identity::{self, Caller};
use jc_app_sdk::sql::{self, Value};
use serde::Deserialize;
use serde_json::Value as Json;

const STATION: &str = "BikeHireDockingStation";
const ATTRS: &str =
    "name,location,availableBikeNumber,freeSlotNumber,totalSlotNumber,status,dateModified";
/// Stations read per page, and at most in all; Helsinki has about 460.
const PAGE: usize = 1000;
const MOST: usize = 5000;
/// How long a sheet's download URL lives; the host allows at most 300 seconds.
const URL_SECONDS: u32 = 120;
/// Ids of stations a plan may name in `include` or `exclude`, and stops a drive may list.
const MOST_IDS: usize = 200;
/// The App role that reads and changes every plan and sets the levels.
const STEWARD: &str = "steward";
const PLANS: Owned = Owned::new("plans", STEWARD);
const DRIVES: Owned = Owned::new("drives", STEWARD);
/// One row, id 1, owned by nobody: the steward role alone changes it.
const LEVELS: Owned = Owned::new("levels", STEWARD);
/// History rows one read answers.
const HISTORY: u32 = 100;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Levels {
    pub low: f64,
    pub high: f64,
    pub target: f64,
}

/// Levels a planner can work with: shares with `0 <= low < target < high <= 1`.
pub fn levels(levels: &Levels) -> Result<(), String> {
    let Levels { low, high, target } = *levels;
    if ![low, high, target].iter().all(|v| v.is_finite()) {
        return Err("the levels are numbers between 0 and 1".into());
    }
    if !(0.0 <= low && low < target && target < high && high <= 1.0) {
        return Err("the levels need 0 ≤ empty below < bring back to < full above ≤ 1".into());
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NewPlan {
    pub operator: String,
    #[serde(default = "van_default")]
    pub van_capacity: u32,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

fn van_default() -> u32 {
    20
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drive {
    pub stops: Vec<String>,
    #[serde(default)]
    pub km: Option<f64>,
    #[serde(default)]
    pub note: Option<String>,
}

/// An operator's name: a van or a crew, 1 to 60 characters once trimmed.
pub fn operator(name: &str) -> Result<String, String> {
    let name = name.trim();
    match name.chars().count() {
        0 => Err("name the van or crew the plan is for".into()),
        n if n > 60 => Err("an operator's name has at most 60 characters".into()),
        _ if name.chars().any(char::is_control) => {
            Err("an operator's name is one line of text".into())
        }
        _ => Ok(name.to_owned()),
    }
}

/// A list of station ids as given: at most [`MOST_IDS`], each 1 to 256 characters, no repeats.
pub fn ids(list: &[String], what: &str) -> Result<Vec<String>, String> {
    if list.len() > MOST_IDS {
        return Err(format!("{what} names at most {MOST_IDS} stations"));
    }
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    for id in list {
        let id = id.trim();
        if id.is_empty() || id.len() > 256 {
            return Err(format!("{what} has a station id that is empty or too long"));
        }
        if !out.iter().any(|seen| seen == id) {
            out.push(id.to_owned());
        }
    }
    Ok(out)
}

/// A drive as it is kept.
#[derive(Debug, PartialEq)]
pub struct Driven {
    pub stops: Vec<String>,
    pub km: Option<f64>,
    pub note: Option<String>,
}

/// A drive as recorded: its stops are stops of the plan, its kilometres plausible, its note short.
pub fn drive(drive: Drive, planned: &[String]) -> Result<Driven, String> {
    let stops = ids(&drive.stops, "a drive")?;
    if stops.is_empty() {
        return Err("a drive names at least one station the van reached".into());
    }
    if let Some(stray) = stops.iter().find(|id| !planned.contains(id)) {
        return Err(format!("{stray} is not a stop of this plan"));
    }
    let km = match drive.km {
        Some(km) if !km.is_finite() || !(0.0..=2000.0).contains(&km) => {
            return Err("a drive's kilometres are between 0 and 2000".into())
        }
        km => km,
    };
    let note = match drive.note.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(note) if note.chars().count() > 500 => {
            return Err("a note has at most 500 characters".into())
        }
        Some(note) => Some(note.to_owned()),
    };
    Ok(Driven { stops, km, note })
}

/// A property's value, keyValues or normalized alike.
fn value(entity: &Json, name: &str) -> Option<Json> {
    let v = entity.get(name)?;
    match v.get("value") {
        Some(inner) if v.get("type").is_some() => Some(inner.clone()),
        _ => Some(v.clone()),
    }
}

fn count(entity: &Json, name: &str) -> Option<u32> {
    value(entity, name)?
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
}

fn text(entity: &Json, name: &str) -> String {
    match value(entity, name) {
        Some(Json::String(s)) => s,
        // A language map: Finnish first, as the page reads it.
        Some(Json::Object(map)) => ["fi", "en", "sv"]
            .iter()
            .find_map(|l| map.get(*l).and_then(Json::as_str))
            .or_else(|| map.values().find_map(Json::as_str))
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

/// A station as the page reads it (src/stations.ts `stationsOf`): out of service, its counts are
/// unknown and it is never routed; a missing count is never read as zero.
pub fn station(entity: &Json) -> Option<Station> {
    let id = entity.get("id")?.as_str()?.to_owned();
    let status = text(entity, "status");
    let working = matches!(status.trim(), "" | "working");
    let at = value(entity, "location").and_then(|l| {
        let c = l.get("coordinates")?.as_array()?;
        Some((c.first()?.as_f64()?, c.get(1)?.as_f64()?))
    });
    let name = text(entity, "name");
    Some(Station {
        name: if name.trim().is_empty() {
            id.rsplit(':').next().unwrap_or(&id).to_owned()
        } else {
            name.trim().to_owned()
        },
        lon: at.map(|a| a.0),
        lat: at.map(|a| a.1),
        bikes: working
            .then(|| count(entity, "availableBikeNumber"))
            .flatten(),
        free: working.then(|| count(entity, "freeSlotNumber")).flatten(),
        capacity: working.then(|| count(entity, "totalSlotNumber")).flatten(),
        id,
    })
}

/// Every station the caller may read from the App's own Endpoint, page by page.
fn stations() -> Result<Vec<Station>, Response> {
    let mut all = Vec::new();
    while all.len() < MOST {
        let path = format!(
            "/ngsi-ld/v1/entities?type={STATION}&options=keyValues&attrs={}&limit={PAGE}&offset={}",
            gateway::encode(ATTRS),
            all.len()
        );
        let page: Vec<Json> = gateway::get_json(&path).map_err(Response::from)?;
        let n = page.len();
        all.extend(page.iter().filter_map(station));
        if n < PAGE {
            break;
        }
    }
    Ok(all)
}

/// One cell of the sheet: quoted when it holds a separator, a quote or a line break, and never
/// read as a formula by a spreadsheet.
pub fn cell(text: &str) -> String {
    let text = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text.to_owned()
    };
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

/// The route sheet: one line per stop, what to do there and what is on board after.
pub fn sheet(operator: &str, stops: &[Stop]) -> String {
    let mut out = format!("# {}\n", cell(operator));
    out.push_str("stop,station,name,action,bikes,load_after,leg_km\n");
    for (n, stop) in stops.iter().enumerate() {
        let action = match stop.action {
            bike_rebalancing::Action::Pick => "take",
            bike_rebalancing::Action::Drop => "leave",
        };
        out.push_str(&format!(
            "{},{},{},{action},{},{},{:.2}\n",
            n + 1,
            cell(&stop.id),
            cell(&stop.name),
            stop.bikes,
            stop.load,
            stop.leg_km
        ));
    }
    out
}

fn id(params: &Params) -> Result<i64, Response> {
    params["id"]
        .parse()
        .map_err(|_| Response::problem(404, "Not Found", "no such plan"))
}

fn bad(why: &str) -> Response {
    Response::problem(400, "Bad Request", why)
}

/// The signed-in caller and the plan id of the address.
fn caller_and_id(params: &Params) -> Result<(Caller, i64), Response> {
    let caller = Caller::require()?;
    Ok((caller, id(params)?))
}

/// The caller, or `null` when signed out: what the page shows and which controls it offers. The
/// server decides again on every write, whatever the page offered.
fn me(_: &Request, _: &Params) -> Response {
    match identity::caller() {
        Some(caller) => Response::json(
            200,
            &serde_json::json!({"id": caller.id, "name": caller.name, "roles": caller.roles}),
        ),
        None => Response::json(200, &Json::Null),
    }
}

/// The levels as kept, or the planner's defaults when the row is missing.
fn kept_levels() -> Result<Levels, Response> {
    let rows = sql::query("select low, high, target from levels where id = 1", &[])
        .map_err(Response::from_sql)?;
    let row = sql::objects(&rows).into_iter().next();
    let read = |name: &str, default: f64| {
        row.as_ref()
            .and_then(|r| r.get(name))
            .and_then(Json::as_f64)
            .unwrap_or(default)
    };
    let fallback = Settings::default();
    Ok(Levels {
        low: read("low", fallback.low),
        high: read("high", fallback.high),
        target: read("target", fallback.target),
    })
}

fn show_levels(_: &Request, _: &Params) -> Response {
    match kept_levels() {
        Ok(Levels { low, high, target }) => Response::json(
            200,
            &serde_json::json!({"low": low, "high": high, "target": target}),
        ),
        Err(answer) => answer,
    }
}

/// The role is checked before the body is read, so nothing in the body stands in for it.
fn change_levels(request: &Request, _: &Params) -> Response {
    let caller = match Caller::require().and_then(|c| c.require_role(&[STEWARD]).map(|()| c)) {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let wanted: Levels = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    if let Err(why) = levels(&wanted) {
        return bad(&why);
    }
    let summary = format!(
        "levels: empty below {:.2}, bring back to {:.2}, full above {:.2}",
        wanted.low, wanted.target, wanted.high
    );
    match LEVELS.update(
        &caller,
        1,
        &["low", "high", "target"],
        vec![
            Value::from(wanted.low),
            Value::from(wanted.high),
            Value::from(wanted.target),
        ],
        &summary,
    ) {
        Ok(()) => show_levels(request, &Params::new()),
        Err(answer) => answer,
    }
}

fn list(request: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let columns = "p.id, p.operator, p.van_capacity as \"vanCapacity\", p.km, p.moved, \
        jsonb_array_length(p.stops) as \"stopCount\", p.created_at as \"createdAt\", \
        (p.owner_id = $1) as mine, \
        (select count(*) from drives d where d.plan_id = p.id) as drives";
    let mut params = vec![
        Value::from(caller.id.as_str()),
        Value::Boolean(caller.has_role(STEWARD)),
    ];
    let operator = request
        .param("operator")
        .map(str::trim)
        .filter(|o| !o.is_empty());
    let only = if let Some(name) = operator {
        params.push(Value::from(name));
        " and p.operator = $3"
    } else {
        ""
    };
    match sql::query(
        &format!(
            "select {columns} from plans p where (p.owner_id = $1 or $2::boolean){only} \
             order by p.id desc limit 50"
        ),
        &params,
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

fn create(request: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let wanted: NewPlan = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let operator = match operator(&wanted.operator) {
        Ok(name) => name,
        Err(why) => return bad(&why),
    };
    if !(1..=200).contains(&wanted.van_capacity) {
        return bad("the van carries 1 to 200 bikes");
    }
    let (include, exclude) = match (
        ids(&wanted.include, "include"),
        ids(&wanted.exclude, "exclude"),
    ) {
        (Ok(i), Ok(e)) => (i, e),
        (Err(why), _) | (_, Err(why)) => return bad(&why),
    };
    let kept = match kept_levels() {
        Ok(kept) => kept,
        Err(answer) => return answer,
    };
    let stations = match stations() {
        Ok(stations) => stations,
        Err(answer) => return answer,
    };
    if stations.is_empty() {
        return Response::problem(
            409,
            "Conflict",
            "the gateway gave no stations to plan over; try again in a minute",
        );
    }
    let start = match wanted
        .start
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(sid) => match stations.iter().find(|s| s.id == sid) {
            Some(Station {
                lon: Some(lon),
                lat: Some(lat),
                ..
            }) => Some((sid.to_owned(), [*lon, *lat])),
            _ => return bad("the start is not a station with a place on the map"),
        },
    };
    let output = run(&Input {
        stations,
        settings: Settings {
            low: kept.low,
            high: kept.high,
            target: kept.target,
            van_capacity: wanted.van_capacity,
            start: start.as_ref().map(|s| s.1),
            include: include.clone(),
            exclude: exclude.clone(),
        },
    });
    let route = output.route;
    let stops = serde_json::to_string(&route.stops).unwrap_or_else(|_| "[]".into());
    let saved = PLANS.insert(
        &caller,
        &[
            "operator",
            "van_capacity",
            "start_station",
            "include",
            "exclude",
            "stops",
            "km",
            "moved",
        ],
        vec![
            Value::from(operator.clone()),
            Value::from(i64::from(wanted.van_capacity)),
            Value::from(start.map(|s| s.0)),
            Value::Json(serde_json::to_string(&include).unwrap_or_else(|_| "[]".into())),
            Value::Json(serde_json::to_string(&exclude).unwrap_or_else(|_| "[]".into())),
            Value::Json(stops),
            Value::from(route.km),
            Value::from(i64::from(route.moved)),
        ],
        &format!("saved a plan for {operator}"),
    );
    let row = match saved {
        Ok(rows) => sql::objects(&rows).into_iter().next().unwrap_or_default(),
        Err(answer) => return answer,
    };
    let Some(plan_id) = row.get("id").and_then(Json::as_i64) else {
        return Response::problem(500, "Internal Server Error", "the plan was not saved");
    };
    let key = format!("sheets/{plan_id}.csv");
    let sheet_saved = blob::put(
        &key,
        sheet(&operator, &route.stops).as_bytes(),
        Some("text/csv; charset=utf-8"),
    )
    .and_then(|()| {
        sql::execute(
            "update plans set sheet = $1 where id = $2",
            &[Value::from(key.clone()), Value::from(plan_id)],
        )
        .map(|_| ())
        .map_err(|err| blob::Error::Unavailable(format!("{err:?}")))
    });
    if let Err(err) = sheet_saved {
        // The plan without its sheet is no plan to drive: both go, the history says so, and the
        // caller hears why.
        let _ = PLANS.delete(&caller, plan_id, "the route sheet could not be saved");
        let _ = blob::delete(&key);
        return Response::from_blob(err);
    }
    Response::json(
        201,
        &serde_json::json!({
            "id": plan_id,
            "operator": operator,
            "vanCapacity": wanted.van_capacity,
            "km": route.km,
            "moved": route.moved,
            "stops": route.stops,
            "createdAt": row.get("created_at"),
            "mine": true,
            "drives": [],
        }),
    )
}

/// The plan's row as JSON when the caller owns it or is a steward, else the 404.
fn plan_row(caller: &Caller, id: i64) -> Result<serde_json::Map<String, Json>, Response> {
    let rows = sql::query(
        "select id, operator, van_capacity as \"vanCapacity\", start_station as \"start\", include, exclude, \
         stops, km, moved, sheet, created_at as \"createdAt\", (owner_id = $2) as mine from plans \
         where id = $1 and (owner_id = $2 or $3::boolean)",
        &[
            Value::from(id),
            Value::from(caller.id.as_str()),
            Value::Boolean(caller.has_role(STEWARD)),
        ],
    )
    .map_err(Response::from_sql)?;
    sql::objects(&rows)
        .into_iter()
        .next()
        .ok_or_else(|| Response::problem(404, "Not Found", "no such plan"))
}

fn show(_: &Request, params: &Params) -> Response {
    let (caller, id) = match caller_and_id(params) {
        Ok(found) => found,
        Err(answer) => return answer,
    };
    let mut plan = match plan_row(&caller, id) {
        Ok(plan) => plan,
        Err(answer) => return answer,
    };
    plan.remove("sheet");
    match sql::query(
        "select id, stops, km, note, driven_at as \"drivenAt\" from drives where plan_id = $1 order by id desc limit 100",
        &[Value::from(id)],
    ) {
        Ok(rows) => {
            plan.insert("drives".into(), Json::from(sql::objects(&rows)));
            Response::json(200, &plan)
        }
        Err(err) => Response::from_sql(err),
    }
}

fn remove(_: &Request, params: &Params) -> Response {
    let (caller, id) = match caller_and_id(params) {
        Ok(found) => found,
        Err(answer) => return answer,
    };
    let plan = match plan_row(&caller, id) {
        Ok(plan) => plan,
        Err(answer) => return answer,
    };
    if let Some(key) = plan.get("sheet").and_then(Json::as_str) {
        match blob::delete(key) {
            Ok(()) | Err(blob::Error::NotFound) => {}
            Err(err) => return Response::from_blob(err),
        }
    }
    let operator = plan
        .get("operator")
        .and_then(Json::as_str)
        .unwrap_or_default();
    match PLANS.delete(&caller, id, &format!("removed the plan for {operator}")) {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Handover {
    operator: String,
}

fn hand_over(request: &Request, params: &Params) -> Response {
    let (caller, id) = match caller_and_id(params)
        .and_then(|(c, id)| c.require_role(&[STEWARD]).map(|()| (c, id)))
    {
        Ok(found) => found,
        Err(answer) => return answer,
    };
    let wanted: Handover = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let operator = match operator(&wanted.operator) {
        Ok(name) => name,
        Err(why) => return bad(&why),
    };
    match PLANS.update(
        &caller,
        id,
        &["operator"],
        vec![Value::from(operator.clone())],
        &format!("handed the plan to {operator}"),
    ) {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

fn record(request: &Request, params: &Params) -> Response {
    let (caller, id) = match caller_and_id(params) {
        Ok(found) => found,
        Err(answer) => return answer,
    };
    let wanted: Drive = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let plan = match plan_row(&caller, id) {
        Ok(plan) => plan,
        Err(answer) => return answer,
    };
    let planned: Vec<String> = plan
        .get("stops")
        .and_then(Json::as_array)
        .map(|stops| {
            stops
                .iter()
                .filter_map(|s| s.get("id")?.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let Driven { stops, km, note } = match drive(wanted, &planned) {
        Ok(drive) => drive,
        Err(why) => return bad(&why),
    };
    let summary = format!("recorded a drive of {} stops on plan {id}", stops.len());
    match DRIVES.insert(
        &caller,
        &["plan_id", "stops", "km", "note"],
        vec![
            Value::from(id),
            Value::Json(serde_json::to_string(&stops).unwrap_or_else(|_| "[]".into())),
            Value::from(km),
            Value::from(note),
        ],
        &summary,
    ) {
        Ok(rows) => {
            let row = sql::objects(&rows).into_iter().next().unwrap_or_default();
            Response::json(
                201,
                &serde_json::json!({
                    "id": row.get("id"),
                    "stops": row.get("stops"),
                    "km": row.get("km"),
                    "note": row.get("note"),
                    "drivenAt": row.get("driven_at"),
                }),
            )
        }
        Err(answer) => answer,
    }
}

fn download(_: &Request, params: &Params) -> Response {
    let (caller, id) = match caller_and_id(params) {
        Ok(found) => found,
        Err(answer) => return answer,
    };
    let plan = match plan_row(&caller, id) {
        Ok(plan) => plan,
        Err(answer) => return answer,
    };
    let Some(key) = plan.get("sheet").and_then(Json::as_str) else {
        return Response::problem(404, "Not Found", "the plan has no route sheet");
    };
    match blob::presign(key, Method::Get, URL_SECONDS) {
        Ok(url) => Response::json(
            200,
            &serde_json::json!({"url": url, "expiresIn": URL_SECONDS}),
        ),
        Err(err) => Response::from_blob(err),
    }
}

/// The newest changes: a steward's every one, a viewer's their own.
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
            Value::Int(i64::from(HISTORY)),
        ],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

pub fn handle(request: Request) -> Response {
    Router::new()
        .get("/api/me", me)
        .get("/api/levels", show_levels)
        .put("/api/levels", change_levels)
        .get("/api/plans", list)
        .post("/api/plans", create)
        .get("/api/plans/{id}", show)
        .delete("/api/plans/{id}", remove)
        .put("/api/plans/{id}/operator", hand_over)
        .post("/api/plans/{id}/drives", record)
        .get("/api/plans/{id}/sheet", download)
        .get("/api/history", history)
        .handle(&request)
}

jc_app_sdk::app!(handle);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn levels_keep_their_order_between_zero_and_one() {
        let ok = Levels {
            low: 0.15,
            high: 0.85,
            target: 0.5,
        };
        assert_eq!(levels(&ok), Ok(()));
        assert_eq!(
            levels(&Levels {
                low: 0.0,
                high: 1.0,
                target: 0.5
            }),
            Ok(())
        );
        for (low, target, high) in [
            (0.5, 0.5, 0.9),
            (0.1, 0.9, 0.9),
            (-0.1, 0.5, 0.9),
            (0.1, 0.5, 1.1),
            (0.6, 0.5, 0.9),
            (f64::NAN, 0.5, 0.9),
            (0.1, f64::INFINITY, 0.9),
        ] {
            assert!(
                levels(&Levels { low, high, target }).is_err(),
                "{low} {target} {high}"
            );
        }
        assert!(serde_json::from_value::<Levels>(
            json!({"low": 0.1, "high": 0.9, "target": 0.5, "roles": ["steward"]})
        )
        .is_err());
    }

    #[test]
    fn an_operator_is_one_short_line() {
        assert_eq!(operator("  Van 2 "), Ok("Van 2".into()));
        assert!(operator("   ").is_err());
        assert!(operator(&"x".repeat(61)).is_err());
        assert_eq!(operator(&"ä".repeat(60)).map(|o| o.chars().count()), Ok(60));
        assert!(operator("Van\n2").is_err());
    }

    #[test]
    fn station_ids_are_bounded_and_kept_once() {
        let list = vec![" a ".to_owned(), "a".to_owned(), "b".to_owned()];
        assert_eq!(
            ids(&list, "include"),
            Ok(vec!["a".to_owned(), "b".to_owned()])
        );
        assert!(ids(&[String::new()], "include").is_err());
        assert!(ids(&["x".repeat(257)], "include").is_err());
        assert!(ids(&vec!["a".to_owned(); MOST_IDS + 1], "include").is_err());
        assert_eq!(ids(&[], "include"), Ok(vec![]));
    }

    #[test]
    fn a_drive_reaches_stops_of_its_plan_only() {
        let planned = vec!["s1".to_owned(), "s2".to_owned()];
        let ok = drive(
            Drive {
                stops: vec!["s2".into(), "s1".into()],
                km: Some(4.5),
                note: Some("  rain ".into()),
            },
            &planned,
        );
        assert_eq!(
            ok,
            Ok(Driven {
                stops: vec!["s2".into(), "s1".into()],
                km: Some(4.5),
                note: Some("rain".into())
            })
        );
        assert!(drive(
            Drive {
                stops: vec![],
                km: None,
                note: None
            },
            &planned
        )
        .is_err());
        assert!(drive(
            Drive {
                stops: vec!["s9".into()],
                km: None,
                note: None
            },
            &planned
        )
        .is_err());
        assert!(drive(
            Drive {
                stops: vec!["s1".into()],
                km: Some(-1.0),
                note: None
            },
            &planned
        )
        .is_err());
        assert!(drive(
            Drive {
                stops: vec!["s1".into()],
                km: Some(f64::NAN),
                note: None
            },
            &planned
        )
        .is_err());
        assert!(drive(
            Drive {
                stops: vec!["s1".into()],
                km: None,
                note: Some("n".repeat(501))
            },
            &planned
        )
        .is_err());
        assert_eq!(
            drive(
                Drive {
                    stops: vec!["s1".into()],
                    km: None,
                    note: Some("  ".into())
                },
                &planned
            )
            .map(|d| d.note),
            Ok(None)
        );
    }

    #[test]
    fn a_station_reads_as_the_page_reads_it() {
        let working = json!({"id": "urn:ngsi-ld:BikeHireDockingStation:042", "type": STATION, "name": "Kamppi",
            "location": {"type": "Point", "coordinates": [24.93, 60.17]},
            "availableBikeNumber": 3, "freeSlotNumber": 7, "totalSlotNumber": 10, "status": "working"});
        let s = station(&working).expect("a station");
        assert_eq!(
            (s.name.as_str(), s.lon, s.lat, s.bikes, s.free, s.capacity),
            (
                "Kamppi",
                Some(24.93),
                Some(60.17),
                Some(3),
                Some(7),
                Some(10)
            )
        );

        let normalized = json!({"id": "urn:ngsi-ld:BikeHireDockingStation:043",
            "name": {"type": "Property", "value": "Töölö"},
            "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [24.9, 60.18]}},
            "availableBikeNumber": {"type": "Property", "value": 0}});
        let s = station(&normalized).expect("a station");
        assert_eq!(
            (s.name.as_str(), s.bikes, s.free, s.capacity),
            ("Töölö", Some(0), None, None)
        );

        let closed = json!({"id": "urn:ngsi-ld:BikeHireDockingStation:044", "status": "outOfService", "availableBikeNumber": 5, "totalSlotNumber": 10});
        let s = station(&closed).expect("a station");
        assert_eq!(
            (s.bikes, s.capacity, s.lon, s.name.as_str()),
            (None, None, None, "044")
        );

        let broken = json!({"id": "urn:ngsi-ld:BikeHireDockingStation:045", "availableBikeNumber": -1, "totalSlotNumber": "ten"});
        let s = station(&broken).expect("a station");
        assert_eq!((s.bikes, s.capacity), (None, None));
        assert!(station(&json!({"name": "no id"})).is_none());
    }

    #[test]
    fn a_sheet_cell_is_never_a_formula_or_a_broken_line() {
        assert_eq!(cell("Kamppi"), "Kamppi");
        assert_eq!(cell("a,b"), "\"a,b\"");
        assert_eq!(cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(cell("-5"), "'-5");
        assert_eq!(cell("two\nlines"), "\"two\nlines\"");
    }

    #[test]
    fn the_sheet_lists_every_stop_in_order() {
        let input: Input = serde_json::from_value(json!({"stations": [
            {"id": "full", "name": "Full, one", "lon": 24.90, "lat": 60.17, "bikes": 10, "free": 0, "capacity": 10},
            {"id": "empty", "name": "=Empty", "lon": 24.91, "lat": 60.17, "bikes": 0, "free": 10, "capacity": 10}
        ]}))
        .expect("input");
        let route = run(&input).route;
        let text = sheet("Van 1", &route.stops);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "# Van 1");
        assert_eq!(lines[1], "stop,station,name,action,bikes,load_after,leg_km");
        assert_eq!(lines.len(), 2 + route.stops.len());
        assert!(
            lines[2].starts_with("1,full,\"Full, one\",take,5,5,"),
            "{}",
            lines[2]
        );
        assert!(
            lines[3].starts_with("2,empty,'=Empty,leave,5,0,"),
            "{}",
            lines[3]
        );
        assert_eq!(sheet("Van 1", &[]).lines().count(), 2);
    }
}
