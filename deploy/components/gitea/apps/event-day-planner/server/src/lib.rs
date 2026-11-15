//! The server of event-day-planner (T-3347): a day a visitor shares, under a short id a link
//! carries, and its calendar file. The page plans in the browser while the visitor picks; a shared
//! day is planned again here from the events the gateway gives, with the caller's own rights (an
//! anonymous visitor reads what the public role may), so what a link opens is what the city's
//! data said and not what a browser sent.
//!
//! Roles (T-3826, API/07), from `identity::caller()` alone: a day shared while signed in is the
//! sharer's own, listed under "My days" and deleted by them alone; a day shared signed out is
//! nobody's. A steward curates the featured events every visitor sees. Every write leaves a row in
//! `app_audit` in the same statement.
//!
//! | Route | Who | What it does |
//! |---|---|---|
//! | `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
//! | `POST /api/itineraries` `{day, ids, lang}` | anyone | the day planned and kept; its code and its plan |
//! | `GET /api/itineraries/{code}` | anyone with the link | the shared day: its date, its picks and its plan |
//! | `GET /api/itineraries/{code}/ics` | anyone with the link | a URL the calendar file downloads from |
//! | `GET /api/days` | signed in | the days the caller shared, newest first |
//! | `DELETE /api/days/{id}` | the sharer | the day and its calendar file gone |
//! | `GET /api/featured` | anyone | the featured events, in the order added |
//! | `POST /api/featured` `{eventId, note}` | steward | an event featured |
//! | `PUT /api/featured/{id}` `{note}` | steward | its note changed |
//! | `DELETE /api/featured/{id}` | steward | no longer featured |
//! | `GET /api/history` | signed in | the newest changes (a viewer: their own) |

use event_day_planner::{run, Day, Event, Input, Settings};
use jc_app_sdk::access::Owned;
use jc_app_sdk::blob::{self, Method};
use jc_app_sdk::gateway;
use jc_app_sdk::http::{Params, Request, Response, Router};
use jc_app_sdk::identity::{self, Caller};
use jc_app_sdk::sql::{self, Value};
use serde::Deserialize;
use serde_json::Value as Json;

const ATTRS: &str = "name,description,startDate,endDate,eventStatus,address,location,source";
/// The events one day may hold, as the planner plans at most 20.
const MOST_PICKS: usize = 20;
/// How long a calendar file's download URL lives; the host allows at most 300 seconds.
const URL_SECONDS: u32 = 120;
/// Shared days cleared per save, so one save never does unbounded work.
const CLEAR_AT_ONCE: i64 = 20;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
/// The App role that curates the featured events.
const STEWARD: &str = "steward";
/// A day is its sharer's alone: no role edits another person's day.
const DAYS: Owned = Owned::new("itineraries", "");
const FEATURED: Owned = Owned::new("featured", STEWARD);
/// Featured events at most, and the longest note.
const MOST_FEATURED: usize = 50;
const NOTE_CHARS: usize = 200;
/// History rows one read answers.
const HISTORY: i64 = 100;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Feature {
    pub event_id: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    pub note: String,
}

/// A featured event's id: an Event's NGSI-LD id, one line, at most 256 characters.
pub fn event_id(id: &str) -> Result<String, String> {
    let id = id.trim();
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err("name the event by its id, at most 256 characters".into());
    }
    if !id.starts_with("urn:ngsi-ld:Event:") {
        return Err("only an event can be featured".into());
    }
    Ok(id.to_owned())
}

/// A note on a featured event: trimmed, one line, at most [`NOTE_CHARS`] characters.
pub fn note(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.chars().count() > NOTE_CHARS {
        return Err(format!("a note has at most {NOTE_CHARS} characters"));
    }
    if text.chars().any(char::is_control) {
        return Err("a note is one line of text".into());
    }
    Ok(text.to_owned())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Share {
    pub day: String,
    /// The picked events' full ids.
    pub ids: Vec<String>,
    #[serde(default)]
    pub lang: Option<String>,
}

/// Days since 1970-01-01 of a civil date (Hinnant's algorithm).
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of days since 1970-01-01 (Hinnant's algorithm).
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
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

/// The last Sunday of a month, as days since 1970-01-01.
fn last_sunday(year: i64, month: i64) -> i64 {
    let (y, m) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let last = days_from_civil(y, m, 1) - 1;
    // 1970-01-01 was a Thursday: (days + 4) mod 7 is 0 on a Sunday.
    last - (last + 4).rem_euclid(7)
}

/// Helsinki's offset from UTC at `ms`: two hours, three in summer time, which the EU starts on the
/// last Sunday of March and ends on the last Sunday of October, both at 01:00 UTC.
pub fn helsinki_offset(ms: i64) -> i64 {
    let (year, _, _) = civil_from_days(ms.div_euclid(DAY));
    let begins = last_sunday(year, 3) * DAY + HOUR;
    let ends = last_sunday(year, 10) * DAY + HOUR;
    if (begins..ends).contains(&ms) {
        3 * HOUR
    } else {
        2 * HOUR
    }
}

/// The instants the Helsinki day `YYYY-MM-DD` starts and ends.
pub fn day_bounds(day: &str) -> Result<(i64, i64), String> {
    let wrong = || format!("{day} is not a day: write it as YYYY-MM-DD");
    let mut parts = day.split('-');
    let (Some(y), Some(m), Some(d), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(wrong());
    };
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return Err(wrong());
    }
    let (y, m, d): (i64, i64, i64) = match (y.parse(), m.parse(), d.parse()) {
        (Ok(y), Ok(m), Ok(d)) => (y, m, d),
        _ => return Err(wrong()),
    };
    let days = days_from_civil(y, m, d);
    // A date like 02-30 rolls into March: it is no day.
    if civil_from_days(days) != (y, m, d) {
        return Err(wrong());
    }
    // Midnight is never inside a clock change (01:00 UTC is 03:00 or 04:00 in Helsinki).
    let midnight = |days: i64| days * DAY - helsinki_offset(days * DAY - 3 * HOUR);
    Ok((midnight(days), midnight(days + 1)))
}

/// The ids of a share: 1 to [`MOST_PICKS`] Event URNs, each kept once.
pub fn ids(list: &[String]) -> Result<Vec<String>, String> {
    if list.is_empty() {
        return Err("pick at least one event to share the day".into());
    }
    if list.len() > MOST_PICKS {
        return Err(format!("a shared day holds at most {MOST_PICKS} events"));
    }
    let mut out: Vec<String> = Vec::new();
    for id in list {
        let id = id.trim();
        let rest = id.strip_prefix("urn:ngsi-ld:Event:").unwrap_or_default();
        if rest.is_empty()
            || id.len() > 256
            || !rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._~-:".contains(c))
        {
            return Err(format!("{id} is not the id of an event"));
        }
        if !out.iter().any(|seen| seen == id) {
            out.push(id.to_owned());
        }
    }
    Ok(out)
}

/// The end of an event's id, as the page keeps it in `?pick=`.
pub fn local(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}

fn value(entity: &Json, name: &str) -> Option<Json> {
    let v = entity.get(name)?;
    match v.get("value").or_else(|| v.get("languageMap")) {
        Some(inner) if v.get("type").is_some() => Some(inner.clone()),
        _ => Some(v.clone()),
    }
}

/// A text attribute in `lang`, a language map read in that language first.
pub fn text(entity: &Json, name: &str, lang: &str) -> String {
    match value(entity, name) {
        Some(Json::String(s)) => s.trim().to_owned(),
        Some(Json::Object(map)) => [lang, "fi", "en", "sv"]
            .iter()
            .find_map(|l| map.get(*l).and_then(Json::as_str))
            .or_else(|| map.values().find_map(Json::as_str))
            .unwrap_or_default()
            .trim()
            .to_owned(),
        _ => String::new(),
    }
}

/// Epoch milliseconds of an RFC 3339 instant (`2026-10-10T08:00:00Z`, `…+03:00`, fractions allowed).
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

/// An event as the page reads it (src/events.ts `planEvents`), its window cut to the day.
pub fn event(entity: &Json, (start, end): (i64, i64), lang: &str) -> Option<Event> {
    let id = entity.get("id")?.as_str()?;
    let first = value(entity, "startDate").and_then(|v| v.as_str().and_then(instant));
    let last = value(entity, "endDate").and_then(|v| v.as_str().and_then(instant));
    let at = value(entity, "location").and_then(|l| {
        let c = l.get("coordinates")?.as_array()?;
        Some((c.first()?.as_f64()?, c.get(1)?.as_f64()?))
    });
    let name = text(entity, "name", lang);
    Some(Event {
        id: local(id).to_owned(),
        name: if name.is_empty() {
            local(id).to_owned()
        } else {
            name
        },
        address: text(entity, "address", lang),
        start: first.map(|f| f.max(start)),
        end: first.map(|f| last.unwrap_or(f).max(f).min(end)),
        lon: at.map(|a| a.0),
        lat: at.map(|a| a.1),
    })
}

/// Whether an event takes place during the day, as the page decides it (src/events.ts `during`).
fn during(entity: &Json, (start, end): (i64, i64)) -> bool {
    let Some(first) = value(entity, "startDate").and_then(|v| v.as_str().and_then(instant)) else {
        return false;
    };
    let last = value(entity, "endDate")
        .and_then(|v| v.as_str().and_then(instant))
        .unwrap_or(first)
        .max(first);
    first < end && (last > start || (last == first && first >= start))
}

/// A share's code: 12 letters and digits of a random UUID Postgres makes.
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

/// The shared days a week past their date, at most [`CLEAR_AT_ONCE`] at a time, files first.
fn clear_old() {
    let Ok(rows) = sql::query(
        "select code from itineraries where day < current_date - 7 order by day limit $1",
        &[Value::from(CLEAR_AT_ONCE)],
    ) else {
        return;
    };
    for row in &rows.values {
        if let Some(Value::Text(code)) = row.first() {
            if matches!(
                blob::delete(&format!("shares/{code}.ics")),
                Ok(()) | Err(blob::Error::NotFound)
            ) {
                let _ = sql::execute(
                    "delete from itineraries where code = $1",
                    &[Value::from(code.as_str())],
                );
            }
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn bad(why: &str) -> Response {
    Response::problem(400, "Bad Request", why)
}

fn share(request: &Request, _: &Params) -> Response {
    let wanted: Share = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let bounds = match day_bounds(wanted.day.trim()) {
        Ok(bounds) => bounds,
        Err(why) => return bad(&why),
    };
    if bounds.1 <= now() - DAY {
        return bad("that day is over; share a day from today on");
    }
    let lang = match wanted.lang.as_deref().unwrap_or("fi") {
        l @ ("fi" | "en" | "sv") => l,
        _ => return bad("the language is fi, en or sv"),
    };
    let ids = match ids(&wanted.ids) {
        Ok(ids) => ids,
        Err(why) => return bad(&why),
    };
    let path = format!(
        "/ngsi-ld/v1/entities?type=Event&options=keyValues&attrs={}&limit={MOST_PICKS}&id={}",
        gateway::encode(ATTRS),
        gateway::encode(&ids.join(","))
    );
    let entities: Vec<Json> = match gateway::get_json(&path).map_err(Response::from) {
        Ok(entities) => entities,
        Err(answer) => return answer,
    };
    let found: Vec<&Json> = entities.iter().filter(|e| during(e, bounds)).collect();
    if found.is_empty() {
        return Response::problem(
            409,
            "Conflict",
            "none of the picked events takes place that day",
        );
    }
    let events: Vec<Event> = found
        .iter()
        .filter_map(|e| event(e, bounds, lang))
        .collect();
    let picks: Vec<String> = events.iter().map(|e| e.id.clone()).collect();
    let day: Day = run(&Input {
        events,
        settings: Settings {
            chosen: picks.clone(),
            now: now(),
            ..Settings::default()
        },
    });
    clear_old();
    let code = match new_code() {
        Ok(code) => code,
        Err(answer) => return answer,
    };
    let key = format!("shares/{code}.ics");
    if let Err(err) = blob::put(
        &key,
        day.ics.as_bytes(),
        Some("text/calendar; charset=utf-8"),
    ) {
        return Response::from_blob(err);
    }
    // Who shared it: the signed-in caller, or nobody (an empty owner no caller has). The `day`
    // column is a date, so the statement is written here with its cast rather than by `Owned`.
    let caller = identity::caller();
    let (owner, name) = caller.as_ref().map_or(("", "anonymous visitor"), |c| {
        (c.id.as_str(), c.name.as_str())
    });
    let saved = sql::execute(
        "with changed as (insert into itineraries (owner_id, code, day, lang, picks, items, conflicts, walk_km, walk_minutes) \
         values ($1, $4, $5::date, $6, $7, $8, $9, $10, $11) returning id) \
         insert into app_audit (actor_id, actor_name, action, target, summary) \
         select $1, $2, 'insert', 'itineraries/' || changed.id::text, $3 from changed",
        &[
            Value::from(owner),
            Value::from(name),
            Value::from(format!("shared a day of {} events on {}", picks.len(), wanted.day.trim())),
            Value::from(code.as_str()),
            Value::from(wanted.day.trim()),
            Value::from(lang),
            json(&picks),
            json(&day.items),
            json(&day.conflicts),
            Value::from(day.walk_km),
            Value::from(day.walk_minutes),
        ],
    );
    if let Err(err) = saved {
        let _ = blob::delete(&key);
        return Response::from_sql(err);
    }
    Response::json(
        201,
        &serde_json::json!({
            "code": code,
            "mine": caller.is_some(),
            "day": wanted.day.trim(),
            "lang": lang,
            "picks": picks,
            "items": day.items,
            "conflicts": day.conflicts,
            "walkKm": day.walk_km,
            "walkMinutes": day.walk_minutes,
        }),
    )
}

/// A `json` parameter of anything serde writes.
fn json<T: serde::Serialize>(value: &T) -> Value {
    Value::Json(serde_json::to_string(value).unwrap_or_else(|_| "null".into()))
}

fn code(params: &Params) -> Result<&str, Response> {
    let code = params["code"].as_str();
    if is_code(code) {
        Ok(code)
    } else {
        Err(Response::problem(
            404,
            "Not Found",
            "no day is shared under this link",
        ))
    }
}

fn show(_: &Request, params: &Params) -> Response {
    let code = match code(params) {
        Ok(code) => code,
        Err(answer) => return answer,
    };
    let rows = sql::query(
        "select code, day::text as day, lang, picks, items, conflicts, walk_km as \"walkKm\", \
         walk_minutes as \"walkMinutes\", created_at as \"createdAt\" from itineraries where code = $1",
        &[Value::from(code)],
    );
    match rows.map(|rows| sql::objects(&rows).into_iter().next()) {
        Ok(Some(day)) => Response::json(200, &day),
        Ok(None) => Response::problem(
            404,
            "Not Found",
            "no day is shared under this link; it may have been cleared a week after its date",
        ),
        Err(err) => Response::from_sql(err),
    }
}

fn calendar(_: &Request, params: &Params) -> Response {
    let code = match code(params) {
        Ok(code) => code,
        Err(answer) => return answer,
    };
    match sql::query(
        "select 1 as one from itineraries where code = $1",
        &[Value::from(code)],
    ) {
        Ok(rows) if rows.values.is_empty() => {
            return Response::problem(404, "Not Found", "no day is shared under this link")
        }
        Ok(_) => {}
        Err(err) => return Response::from_sql(err),
    }
    match blob::presign(&format!("shares/{code}.ics"), Method::Get, URL_SECONDS) {
        Ok(url) => Response::json(
            200,
            &serde_json::json!({"url": url, "expiresIn": URL_SECONDS}),
        ),
        Err(err) => Response::from_blob(err),
    }
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

fn id(params: &Params, what: &str) -> Result<i64, Response> {
    params["id"]
        .parse()
        .map_err(|_| Response::problem(404, "Not Found", &format!("no such {what}")))
}

/// The days the caller shared, newest first.
fn my_days(_: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    match sql::query(
        "select id, code, day::text as day, jsonb_array_length(picks) as events, \
         created_at as \"createdAt\" from itineraries where owner_id = $1 order by id desc limit 50",
        &[Value::from(caller.id.as_str())],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

fn forget_day(_: &Request, params: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let id = match id(params, "day") {
        Ok(id) => id,
        Err(answer) => return answer,
    };
    let rows = match sql::query(
        "select code from itineraries where id = $1 and owner_id = $2",
        &[Value::from(id), Value::from(caller.id.as_str())],
    ) {
        Ok(rows) => rows,
        Err(err) => return Response::from_sql(err),
    };
    let Some(Value::Text(code)) = rows.values.first().and_then(|r| r.first()) else {
        return Response::problem(404, "Not Found", "no such record, or not yours to change");
    };
    match blob::delete(&format!("shares/{code}.ics")) {
        Ok(()) | Err(blob::Error::NotFound) => {}
        Err(err) => return Response::from_blob(err),
    }
    match DAYS.delete(&caller, id, "deleted a shared day") {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

fn featured(_: &Request, _: &Params) -> Response {
    match sql::query(
        "select id, event_id as \"eventId\", note from featured order by id limit 50",
        &[],
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

fn feature(request: &Request, _: &Params) -> Response {
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let wanted: Feature = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let (event, note) = match (event_id(&wanted.event_id), note(&wanted.note)) {
        (Ok(event), Ok(note)) => (event, note),
        (Err(why), _) | (_, Err(why)) => return bad(&why),
    };
    match sql::query(
        "select count(*) as n, count(*) filter (where event_id = $1) as same from featured",
        &[Value::from(event.as_str())],
    ) {
        Ok(rows) => {
            let row = sql::objects(&rows).into_iter().next().unwrap_or_default();
            let read = |k: &str| row.get(k).and_then(Json::as_i64).unwrap_or(0);
            if read("same") > 0 {
                return Response::problem(409, "Conflict", "that event is featured already");
            }
            if read("n") >= MOST_FEATURED as i64 {
                return Response::problem(
                    409,
                    "Conflict",
                    &format!("at most {MOST_FEATURED} events are featured; remove one first"),
                );
            }
        }
        Err(err) => return Response::from_sql(err),
    }
    match FEATURED.insert(
        &caller,
        &["event_id", "note"],
        vec![Value::from(event.as_str()), Value::from(note.as_str())],
        &format!("featured {}", local(&event)),
    ) {
        Ok(rows) => {
            let row = sql::objects(&rows).into_iter().next().unwrap_or_default();
            Response::json(
                201,
                &serde_json::json!({"id": row.get("id"), "eventId": event, "note": note}),
            )
        }
        Err(answer) => answer,
    }
}

fn change_note(request: &Request, params: &Params) -> Response {
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let id = match id(params, "featured event") {
        Ok(id) => id,
        Err(answer) => return answer,
    };
    let wanted: Note = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let note = match note(&wanted.note) {
        Ok(note) => note,
        Err(why) => return bad(&why),
    };
    match FEATURED.update(
        &caller,
        id,
        &["note"],
        vec![Value::from(note)],
        "changed a featured event's note",
    ) {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

fn unfeature(_: &Request, params: &Params) -> Response {
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let id = match id(params, "featured event") {
        Ok(id) => id,
        Err(answer) => return answer,
    };
    match FEATURED.delete(&caller, id, "no longer features an event") {
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
        .post("/api/itineraries", share)
        .get("/api/itineraries/{code}", show)
        .get("/api/itineraries/{code}/ics", calendar)
        .get("/api/me", me)
        .get("/api/days", my_days)
        .delete("/api/days/{id}", forget_day)
        .get("/api/featured", featured)
        .post("/api/featured", feature)
        .put("/api/featured/{id}", change_note)
        .delete("/api/featured/{id}", unfeature)
        .get("/api/history", history)
        .handle(&request)
}

jc_app_sdk::app!(handle);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_featured_event_is_an_event_and_its_note_one_short_line() {
        assert_eq!(
            event_id(" urn:ngsi-ld:Event:helsinki:fair "),
            Ok("urn:ngsi-ld:Event:helsinki:fair".into())
        );
        assert!(event_id("").is_err());
        assert!(event_id("urn:ngsi-ld:Place:x").is_err());
        assert!(event_id(&format!("urn:ngsi-ld:Event:{}", "x".repeat(250))).is_err());
        assert!(event_id("urn:ngsi-ld:Event:a\nb").is_err());
        assert_eq!(note("  free entry "), Ok("free entry".into()));
        assert_eq!(note(""), Ok(String::new()));
        assert_eq!(note(&"ä".repeat(200)).map(|n| n.chars().count()), Ok(200));
        assert!(note(&"x".repeat(201)).is_err());
        assert!(note("two\nlines").is_err());
        assert!(serde_json::from_value::<Feature>(
            json!({"eventId": "urn:ngsi-ld:Event:a", "roles": ["steward"]})
        )
        .is_err());
    }

    #[test]
    fn helsinki_keeps_summer_time_from_the_last_sunday_of_march_to_that_of_october() {
        // 2026: 29 March and 25 October, at 01:00 UTC.
        let march = days_from_civil(2026, 3, 29) * DAY + HOUR;
        let october = days_from_civil(2026, 10, 25) * DAY + HOUR;
        assert_eq!(helsinki_offset(march - 1), 2 * HOUR);
        assert_eq!(helsinki_offset(march), 3 * HOUR);
        assert_eq!(helsinki_offset(october - 1), 3 * HOUR);
        assert_eq!(helsinki_offset(october), 2 * HOUR);
        assert_eq!(
            helsinki_offset(days_from_civil(2026, 1, 15) * DAY),
            2 * HOUR
        );
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }

    #[test]
    fn a_day_runs_from_helsinki_midnight_to_the_next_even_across_a_clock_change() {
        let (start, end) = day_bounds("2026-10-10").expect("a day");
        assert_eq!(start, days_from_civil(2026, 10, 9) * DAY + 21 * HOUR);
        assert_eq!(end - start, DAY);
        let (start, end) = day_bounds("2026-10-25").expect("the day summer time ends");
        assert_eq!(end - start, 25 * HOUR);
        let (start, end) = day_bounds("2026-03-29").expect("the day it begins");
        assert_eq!(end - start, 23 * HOUR);
        let (start, _) = day_bounds("2026-01-01").expect("winter");
        assert_eq!(start, days_from_civil(2025, 12, 31) * DAY + 22 * HOUR);
        for wrong in [
            "",
            "2026-1-01",
            "2026-02-30",
            "2026-13-01",
            "2026-00-10",
            "26-10-10",
            "2026-10-10T00:00",
            "2026/10/10",
            "2025-02-29",
        ] {
            assert!(day_bounds(wrong).is_err(), "{wrong}");
        }
        assert!(day_bounds("2028-02-29").is_ok());
    }

    #[test]
    fn a_share_names_one_to_twenty_events_by_their_urn() {
        assert_eq!(
            ids(&[
                " urn:ngsi-ld:Event:helsinki:abc ".into(),
                "urn:ngsi-ld:Event:helsinki:abc".into()
            ]),
            Ok(vec!["urn:ngsi-ld:Event:helsinki:abc".into()])
        );
        assert!(ids(&[]).is_err());
        assert!(ids(&vec!["urn:ngsi-ld:Event:x".to_owned(); 21]).is_err());
        for wrong in [
            "urn:ngsi-ld:Place:x",
            "urn:ngsi-ld:Event:",
            "urn:ngsi-ld:Event:a,b",
            "urn:ngsi-ld:Event:a&type=Place",
            "urn:ngsi-ld:Event:a b",
        ] {
            assert!(ids(&[wrong.into()]).is_err(), "{wrong}");
        }
    }

    #[test]
    fn an_instant_reads_in_any_offset() {
        let utc = days_from_civil(2026, 10, 10) * DAY + 8 * HOUR;
        assert_eq!(instant("2026-10-10T08:00:00Z"), Some(utc));
        assert_eq!(instant("2026-10-10T11:00:00+03:00"), Some(utc));
        assert_eq!(instant("2026-10-10T08:00:00.250Z"), Some(utc + 250));
        assert_eq!(instant("2026-10-10T05:30:00-02:30"), Some(utc));
        for wrong in [
            "2026-10-10",
            "noon",
            "2026-10-10T25:00:00Z",
            "2026-10-10T08:00:00",
        ] {
            assert_eq!(instant(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn an_event_reads_as_the_page_reads_it_cut_to_the_day() {
        let bounds = day_bounds("2026-10-10").expect("day");
        let fair = json!({"id": "urn:ngsi-ld:Event:helsinki:fair", "name": {"fi": "Messut", "en": "Fair"},
            "startDate": "2026-10-09T07:00:00Z", "endDate": "2026-10-12T15:00:00Z",
            "location": {"type": "Point", "coordinates": [24.94, 60.17]}, "address": "Messukeskus"});
        let e = event(&fair, bounds, "en").expect("an event");
        assert_eq!(
            (e.id.as_str(), e.name.as_str(), e.address.as_str()),
            ("fair", "Fair", "Messukeskus")
        );
        assert_eq!((e.start, e.end), (Some(bounds.0), Some(bounds.1)));
        assert_eq!(
            event(&fair, bounds, "sv").map(|e| e.name),
            Some("Messut".into()),
            "Finnish when Swedish is missing"
        );
        assert!(during(&fair, bounds));

        let normalized = json!({"id": "urn:ngsi-ld:Event:helsinki:talk", "name": {"type": "LanguageProperty", "languageMap": {"fi": "Puhe"}},
            "startDate": {"type": "Property", "value": "2026-10-10T09:00:00Z"}});
        let e = event(&normalized, bounds, "en").expect("an event");
        assert_eq!((e.name.as_str(), e.lon, e.end), ("Puhe", None, e.start));

        let undated = json!({"id": "urn:ngsi-ld:Event:helsinki:x"});
        assert!(!during(&undated, bounds));
        assert_eq!(
            event(&undated, bounds, "fi").map(|e| (e.name, e.start)),
            Some(("x".into(), None))
        );
        assert!(
            !during(
                &json!({"id": "a", "startDate": "2026-10-11T09:00:00Z"}),
                bounds
            ),
            "the next day"
        );
        assert!(event(&json!({"name": "no id"}), bounds, "fi").is_none());
    }

    #[test]
    fn a_code_is_twelve_lowercase_letters_or_digits() {
        assert!(is_code("abc123def456"));
        for wrong in [
            "",
            "abc",
            "ABC123DEF456",
            "abc123def45/",
            "abc123def4567",
            "../../shares",
        ] {
            assert!(!is_code(wrong), "{wrong}");
        }
    }
}
