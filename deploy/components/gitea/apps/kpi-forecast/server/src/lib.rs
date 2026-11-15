//! The server of kpi-forecast (T-3350): every indicator's forecast kept with the day it was made,
//! so a later visit can set it against what was measured, and a monthly report of the forecasts
//! against the readings as CSV under the App's own prefix. The page still forecasts in the
//! browser for the period on screen; the server's daily job records the day's forecasts from the
//! history it reads itself, page by page, and writes the month's report (T-6197: reading every
//! indicator's history takes longer than a request's 5 s, so no request reads it).
//!
//! | Route | What it does |
//! |---|---|
//! | `POST /api/forecasts` `{days}` | whether the day's forecasts of the period are recorded (200), or 202 until the job runs |
//! | `GET /api/forecasts?kpi=&days=` | the forecasts kept for an indicator, newest first |
//! | `POST /api/reports` `{month}` | the URL of the month's report (this month's by default) the job wrote; 409 until it wrote one |
//!
//! The job `record-forecasts` records every period's forecasts once a day at 03:20 UTC (app.yaml)
//! and writes this month's report, and last month's until a run after its end wrote it.
//!
//! Roles and edits (T-3827, API/07), decided from `identity::caller()` alone:
//!
//! | Route | Who | What it does |
//! |---|---|---|
//! | `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
//! | `GET /api/published?kpi=` | anyone | an indicator's published forecasts, newest first |
//! | `POST /api/published` `{kpi, days, horizon, note}` | steward | a forecast made with these parameters and published |
//! | `DELETE /api/published/{id}` | steward | a published forecast withdrawn |
//! | `GET /api/follows` | signed in | the indicators the caller follows |
//! | `POST /api/follows` `{kpi}` | signed in | an indicator followed, at most 50 |
//! | `DELETE /api/follows/{id}` | the follower | no longer followed |
//! | `GET /api/history` | signed in | the newest 100 changes (a viewer: their own) |

use jc_app_sdk::access::Owned;
use jc_app_sdk::blob::{self, Method};
use jc_app_sdk::gateway;
use jc_app_sdk::http::{Params, Request, Response, Router};
use jc_app_sdk::identity::{self, Caller};
use jc_app_sdk::sql::{self, Value};
use kpi_forecast::{analyse_series, Direction, Point, Series, Status};
use serde::Deserialize;
use serde_json::{json, Value as Json};

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
/// What the App's policies let it read back: 90 days.
const READABLE: i64 = 90 * DAY;
/// The readings one temporal read asks for per indicator, as the page does (src/kpis.ts).
const LAST_N: usize = 2000;
/// Indicators one temporal read asks for, and the most pages one history read walks.
const PAGE: usize = 100;
const MOST_PAGES: usize = 70;
/// How long the job reads history before it gives up, inside its 60 s wall time (wasm-host
/// limits.rs): a page asked for later than this is not asked for.
const JOB_READ_MS: i64 = 40_000;
/// When the job runs (app.yaml), as the answers that wait for it say.
const JOB_AT: &str = "03:20 UTC";
/// Forecasts listed for one indicator.
const LISTED: i64 = 60;
/// How long a report's download URL lives; the host allows at most 300 seconds.
const URL_SECONDS: u32 = 120;

/// The App role that publishes forecasts (app.yaml).
const STEWARD: &str = "steward";
/// A published forecast any steward may withdraw; a follow is its follower's alone.
const PUBLISHED: Owned = Owned::new("published", STEWARD);
const FOLLOWS: Owned = Owned::new("follows", "");
/// The most indicators one person follows, the published forecasts listed, the history read.
const MOST_FOLLOWS: i64 = 50;
const PUBLISHED_LISTED: i64 = 10;
const HISTORY: i64 = 100;
/// The longest forecast a steward publishes, in the model's steps.
pub const MOST_HORIZON: i64 = 48;

/// An indicator's id: its URN, of letters, digits and `._~:-`, at most 256 long.
pub fn kpi_urn(id: &str) -> Result<&str, String> {
    let rest = id
        .strip_prefix("urn:ngsi-ld:KeyPerformanceIndicator:")
        .unwrap_or_default();
    if rest.is_empty()
        || id.len() > 256
        || !rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._~:-".contains(c))
    {
        return Err(format!("{id} is not the id of an indicator"));
    }
    Ok(id)
}

/// The period, 7, 30 or 90 days, as the page offers it.
pub fn period(days: i64) -> Result<i64, String> {
    match days {
        7 | 30 | 90 => Ok(days),
        _ => Err(format!("{days} is not a period: 7, 30 or 90 days")),
    }
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

/// The first and the last instant of a month `YYYY-MM`, UTC: `[start, end)`.
pub fn month_bounds(month: &str) -> Result<(i64, i64), String> {
    let wrong = || format!("{month} is not a month: write it as YYYY-MM");
    let (y, m) = month.split_once('-').ok_or_else(wrong)?;
    if y.len() != 4 || m.len() != 2 {
        return Err(wrong());
    }
    let (y, m): (i64, i64) = (
        y.parse().map_err(|_| wrong())?,
        m.parse().map_err(|_| wrong())?,
    );
    if !(1..=12).contains(&m) {
        return Err(wrong());
    }
    let next = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    Ok((
        days_from_civil(y, m, 1) * DAY,
        days_from_civil(next.0, next.1, 1) * DAY,
    ))
}

/// Each indicator's `currentValue` readings from a temporal answer, as the page reads them.
pub fn series_of(entities: &[Json]) -> Vec<Series> {
    entities
        .iter()
        .filter_map(|entity| {
            let id = entity.get("id")?.as_str()?.to_owned();
            let property = entity.get("currentValue");
            let values = property.and_then(|p| {
                p.get("values")
                    .and_then(Json::as_array)
                    .or_else(|| p.as_array())
            });
            let points = values
                .into_iter()
                .flatten()
                .filter_map(|entry| {
                    let (v, at) = match entry {
                        Json::Array(pair) if pair.len() >= 2 => {
                            (pair[0].as_f64()?, pair[1].as_str()?)
                        }
                        Json::Object(o) => {
                            (o.get("value")?.as_f64()?, o.get("observedAt")?.as_str()?)
                        }
                        _ => return None,
                    };
                    Some(Point {
                        t: instant(at)? as f64,
                        v,
                    })
                })
                .collect();
            Some(Series { id, points })
        })
        .collect()
}

/// One page of the temporal read of `currentValue` from `from` on, the indicators from `offset`;
/// of `kpi` alone when given. `timerel=after`, never `between`: on dev (6,719 indicators,
/// 2026-10-31) `between` timed out at the gateway's 30 s on every try while `after` answered in
/// 1 to 23 s (T-6197). The temporal read lists only the indicators with readings in the window.
pub fn history_path(from: i64, kpi: Option<&str>, offset: usize) -> String {
    let only = kpi.map_or_else(String::new, |id| format!("&id={}", gateway::encode(id)));
    format!(
        "/ngsi-ld/v1/temporal/entities?type=KeyPerformanceIndicator{only}&attrs=currentValue&timerel=after&timeAt={}&lastN={LAST_N}&options=temporalValues&limit={PAGE}&offset={offset}",
        gateway::encode(&iso(from))
    )
}

/// The readings from `from` on, read page by page through `fetch` until a page comes back short;
/// `in_time` is asked before each later page, and the read stops naming how far it came when the
/// time is up.
pub fn read_pages(
    from: i64,
    kpi: Option<&str>,
    mut fetch: impl FnMut(&str) -> Result<Vec<Json>, gateway::Error>,
    mut in_time: impl FnMut() -> bool,
) -> Result<Vec<Series>, Failure> {
    let mut entities = Vec::new();
    // ponytail: at most MOST_PAGES pages, the first 7,000 indicators with readings; a cursor kept
    // across runs when more than that report a value.
    for page in 0..MOST_PAGES {
        if page > 0 && !in_time() {
            return Err(Failure::Late(entities.len()));
        }
        let got = fetch(&history_path(from, kpi, page * PAGE)).map_err(Failure::Gateway)?;
        let short = got.len() < PAGE;
        entities.extend(got);
        if short {
            break;
        }
    }
    Ok(series_of(&entities))
}

/// Each indicator's readings from `from` on.
pub fn since(series: &[Series], from: i64) -> Vec<Series> {
    series
        .iter()
        .map(|s| Series {
            id: s.id.clone(),
            points: s
                .points
                .iter()
                .filter(|p| p.t >= from as f64)
                .map(|p| Point { t: p.t, v: p.v })
                .collect(),
        })
        .collect()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn bad(why: &str) -> Response {
    Response::problem(400, "Bad Request", why)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    days: i64,
}

/// The day's forecast of each indicator with enough history, as rows to keep.
pub fn forecasts_of(series: &[Series]) -> Vec<Json> {
    series
        .iter()
        .filter_map(|s| {
            let result = analyse_series(s, None);
            if result.status != Status::Ok || result.forecast.is_empty() {
                return None;
            }
            let direction = result.trend.as_ref().map(|t| match t.direction {
                Direction::Up => "up",
                Direction::Down => "down",
                Direction::Flat => "flat",
            });
            let last = result.history.last()?;
            Some(json!({"kpi": s.id, "step": result.step?, "latest_t": last[0], "latest_v": last[1], "direction": direction, "points": result.forecast}))
        })
        .collect()
}

/// Why recording or a report stopped: the App's schema, its Endpoint, its store or the time.
pub enum Failure {
    Sql(sql::Error),
    Gateway(gateway::Error),
    Blob(blob::Error),
    /// The history read ran out of time after this many indicators.
    Late(usize),
}

impl Failure {
    fn response(self) -> Response {
        match self {
            Self::Sql(err) => Response::from_sql(err),
            Self::Gateway(err) => Response::from(err),
            Self::Blob(err) => Response::from_blob(err),
            Self::Late(_) => Response::problem(504, "Gateway Timeout", &self.why()),
        }
    }

    /// What failed, never a body or a credential (AP-155).
    fn why(&self) -> String {
        match self {
            Self::Sql(
                sql::Error::Refused(why)
                | sql::Error::Invalid(why)
                | sql::Error::Quota(why)
                | sql::Error::Unavailable(why),
            ) => {
                format!("the App's database said: {why}")
            }
            Self::Sql(sql::Error::Timeout) => "the App's database took too long".to_owned(),
            Self::Gateway(err) => format!("the indicators' history could not be read: {err}"),
            Self::Blob(err) => format!("the App's store said: {err}"),
            Self::Late(read) => format!(
                "the indicators' history took longer than the job may wait, after {read} indicators"
            ),
        }
    }

    /// The sentence a job run ends with for a period.
    fn sentence(&self, days: i64) -> String {
        format!("the {days}-day forecasts were not recorded: {}", self.why())
    }
}

/// Whether today's forecasts of the period are recorded.
fn recorded_today(days: i64) -> Result<bool, sql::Error> {
    sql::query(
        "select 1 as one from forecasts where made_on = current_date and days = $1 limit 1",
        &[Value::from(days)],
    )
    .map(|done| !done.values.is_empty())
}

/// Records today's forecasts for one period from `series`, once a day: how many rows, and whether
/// today's were there already.
fn record_period(days: i64, series: &[Series]) -> Result<(u64, bool), Failure> {
    if recorded_today(days).map_err(Failure::Sql)? {
        return Ok((0, true));
    }
    let rows = forecasts_of(series);
    if rows.is_empty() {
        return Ok((0, false));
    }
    let n = sql::execute(
        "insert into forecasts (kpi, made_on, days, step, latest_t, latest_v, direction, points) \
         select r.kpi, current_date, $1, r.step, r.latest_t, r.latest_v, r.direction, r.points \
         from jsonb_to_recordset($2) as r(kpi text, step double precision, latest_t double precision, latest_v double precision, direction text, points jsonb) \
         where r.kpi like 'urn:ngsi-ld:KeyPerformanceIndicator:%' and length(r.kpi) <= 256 \
         on conflict (kpi, made_on, days) do nothing",
        &[Value::from(days), Value::Json(Json::from(rows).to_string())],
    )
    .map_err(Failure::Sql)?;
    Ok((n, false))
}

/// Whether today's forecasts of the period are recorded. The job records them (`JOB_AT`): reading
/// every indicator's history takes longer than a request may (5 s), so a visit never reads it.
fn record(request: &Request, _: &Params) -> Response {
    let wanted: Record = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let days = match period(wanted.days) {
        Ok(days) => days,
        Err(why) => return bad(&why),
    };
    match recorded_today(days) {
        Ok(true) => Response::json(200, &json!({"recorded": 0, "already": true})),
        Ok(false) => Response::json(
            202,
            &json!({"recorded": 0, "already": false, "detail": format!("the daily job records today's forecasts at {JOB_AT}")}),
        ),
        Err(err) => Response::from_sql(err),
    }
}

/// The periods the page offers, which the daily job records (AP-154).
pub const PERIODS: [i64; 3] = [7, 30, 90];

/// The months whose reports the job writes at `now`: this month, then the last one, which stays
/// open until a run after its end wrote it.
pub fn report_months(now: i64) -> [String; 2] {
    let (y, m, _) = civil_from_days(now.div_euclid(DAY));
    let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    [format!("{y:04}-{m:02}"), format!("{py:04}-{pm:02}")]
}

/// Whether the month's stored report is final, written after the month ended; `None` when none is
/// stored.
fn stored_report(month: &str, end: i64) -> Result<Option<bool>, sql::Error> {
    let rows = sql::query(
        "select written_at >= to_timestamp($2::double precision / 1000) as final from reports where month = $1",
        &[Value::from(month), Value::from(end as f64)],
    )?;
    Ok(sql::objects(&rows)
        .first()
        .map(|row| row.get("final").and_then(Json::as_bool).unwrap_or(false)))
}

/// Writes the month's report from the kept forecasts and `readings` under `reports/{month}.csv`.
fn write_report(month: &str, readings: &[Series]) -> Result<(), Failure> {
    let (start, end) = month_bounds(month).unwrap_or((0, 0));
    let forecasts = sql::query(
        "select kpi, made_on::text as \"madeOn\", days, step, points from forecasts \
         where made_on >= to_timestamp($1::double precision / 1000)::date - 90 and made_on < to_timestamp($2::double precision / 1000)::date \
         order by kpi, made_on, days limit 1000",
        &[Value::from(start as f64), Value::from(end as f64)],
    )
    .map_err(Failure::Sql)?;
    let forecasts: Vec<Json> = sql::objects(&forecasts)
        .into_iter()
        .map(Json::Object)
        .collect();
    blob::put(
        &report_key(month),
        report(month, &forecasts, readings).as_bytes(),
        Some("text/csv; charset=utf-8"),
    )
    .map_err(Failure::Blob)?;
    sql::execute(
        "insert into reports (month) values ($1) on conflict (month) do update set written_at = now()",
        &[Value::from(month)],
    )
    .map_err(Failure::Sql)?;
    Ok(())
}

fn report_key(month: &str) -> String {
    format!("reports/{month}.csv")
}

/// The daily job `record-forecasts` (app.yaml `spec.server.jobs`, T-3372): reads the last 90
/// days of every indicator once, page by page (T-6197), records every period's forecasts of the
/// day and writes this month's report and last month's until it is final. A period already
/// recorded today is left as it is; the run fails naming everything it could not do.
pub fn record_forecasts() -> Result<(), String> {
    let started = now();
    let series = read_pages(started - READABLE + MINUTE, None, gateway::get_json, || {
        now() < started + JOB_READ_MS
    })
    .map_err(|failure| format!("nothing was recorded: {}", failure.why()))?;
    let mut failed: Vec<String> = PERIODS
        .iter()
        .filter_map(|&days| {
            record_period(days, &since(&series, started - days * DAY))
                .err()
                .map(|failure| failure.sentence(days))
        })
        .collect();
    let [this, last] = report_months(started);
    let last_final = month_bounds(&last)
        .ok()
        .map(|(_, end)| stored_report(&last, end));
    let months = match last_final {
        Some(Ok(Some(true))) => vec![this],
        Some(Err(err)) => {
            failed.push(format!(
                "the report of {last} was not written: {}",
                Failure::Sql(err).why()
            ));
            vec![this]
        }
        _ => vec![this, last],
    };
    for month in months {
        if let Err(failure) = write_report(&month, &series) {
            failed.push(format!(
                "the report of {month} was not written: {}",
                failure.why()
            ));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

fn list(request: &Request, _: &Params) -> Response {
    let kpi = match kpi_urn(request.param("kpi").unwrap_or_default()) {
        Ok(kpi) => kpi,
        Err(why) => return bad(&why),
    };
    let days = match request.param("days").map(str::parse::<i64>) {
        None => None,
        Some(Ok(d)) => match period(d) {
            Ok(d) => Some(d),
            Err(why) => return bad(&why),
        },
        Some(Err(_)) => return bad("the period is 7, 30 or 90 days"),
    };
    let rows = sql::query(
        "select made_on::text as \"madeOn\", days, step, latest_t as \"latestT\", latest_v as \"latestV\", direction, points \
         from forecasts where kpi = $1 and ($2::int is null or days = $2::int) order by made_on desc, days limit $3",
        &[Value::from(kpi), Value::from(days), Value::from(LISTED)],
    );
    match rows {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

/// One cell of the report: quoted when it holds a separator, a quote or a line break, and never
/// read as a formula by a spreadsheet.
pub fn cell(text: &str) -> String {
    let text = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) && text.parse::<f64>().is_err()
    {
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

/// The reading nearest `t` within half a step, if any.
pub fn measured_at(series: Option<&Series>, t: f64, step: f64) -> Option<f64> {
    series?
        .points
        .iter()
        .filter(|p| p.v.is_finite() && (p.t - t).abs() <= step / 2.0)
        .min_by(|a, b| (a.t - t).abs().total_cmp(&(b.t - t).abs()))
        .map(|p| p.v)
}

/// The report: every kept forecast point that falls in the month, against the reading at that
/// time when there is one.
pub fn report(month: &str, forecasts: &[Json], readings: &[Series]) -> String {
    let (start, end) = month_bounds(month).unwrap_or((0, 0));
    let mut out = format!("# kpi-forecast: forecasts for {month} against the readings\nkpi,made_on,period_days,for_utc,expected,low_95,high_95,measured,within_95\n");
    for f in forecasts {
        let kpi = f.get("kpi").and_then(Json::as_str).unwrap_or_default();
        let made_on = f.get("madeOn").and_then(Json::as_str).unwrap_or_default();
        let days = f.get("days").and_then(Json::as_i64).unwrap_or_default();
        let step = f
            .get("step")
            .and_then(Json::as_f64)
            .unwrap_or(f64::INFINITY);
        let series = readings.iter().find(|s| s.id == kpi);
        for p in f
            .get("points")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(t), Some(v), Some(lo), Some(hi)) = (
                p["t"].as_f64(),
                p["v"].as_f64(),
                p["lo"].as_f64(),
                p["hi"].as_f64(),
            ) else {
                continue;
            };
            if !((start as f64)..(end as f64)).contains(&t) {
                continue;
            }
            let measured = measured_at(series, t, step);
            out.push_str(&format!(
                "{},{},{days},{},{v},{lo},{hi},{},{}\n",
                cell(kpi),
                cell(made_on),
                iso(t as i64),
                measured.map_or(String::new(), |m| m.to_string()),
                measured.map_or("", |m| if (lo..=hi).contains(&m) { "yes" } else { "no" }),
            ));
        }
    }
    out
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Month {
    /// `YYYY-MM`; this month (UTC) when absent.
    #[serde(default)]
    month: Option<String>,
}

fn monthly(request: &Request, _: &Params) -> Response {
    let wanted: Month = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let now = now();
    let this_month = {
        let (y, m, _) = civil_from_days(now.div_euclid(DAY));
        format!("{y:04}-{m:02}")
    };
    let month = wanted
        .month
        .as_deref()
        .map_or(this_month.as_str(), str::trim);
    let (start, end) = match month_bounds(month) {
        Ok(bounds) => bounds,
        Err(why) => return bad(&why),
    };
    if start > now {
        return bad("that month has not begun");
    }
    if end <= now - READABLE {
        return bad("a report covers a month of the last 90 days, the history the App may read");
    }
    // The job writes the report (`record_forecasts`): the readings take longer than a request may.
    let final_report = match stored_report(month, end) {
        Ok(Some(final_report)) => final_report,
        Ok(None) => {
            return Response::problem(
                409,
                "Conflict",
                &format!("the report of {month} is not written yet: the daily job writes this month's report at {JOB_AT}, and last month's until it is final"),
            )
        }
        Err(err) => return Response::from_sql(err),
    };
    match blob::presign(&report_key(month), Method::Get, URL_SECONDS) {
        Ok(url) => Response::json(
            201,
            &json!({"month": month, "url": url, "expiresIn": URL_SECONDS, "final": final_report}),
        ),
        Err(err) => Response::from_blob(err),
    }
}

/// A forecast to publish, as the page sends it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publish {
    pub kpi: String,
    pub days: i64,
    pub horizon: i64,
    pub note: String,
}

/// The note, trimmed, of 1 to 200 characters on one line, and the horizon in steps: or why not.
pub fn checked_publish(publish: &Publish) -> Result<(String, usize), String> {
    kpi_urn(&publish.kpi)?;
    period(publish.days)?;
    if !(1..=MOST_HORIZON).contains(&publish.horizon) {
        return Err(format!("the horizon is 1 to {MOST_HORIZON} steps"));
    }
    let note = publish.note.trim();
    if note.is_empty() || note.chars().count() > 200 || note.chars().any(char::is_control) {
        return Err("a note is 1 to 200 characters, on one line".to_owned());
    }
    Ok((
        note.to_owned(),
        usize::try_from(publish.horizon).unwrap_or(1),
    ))
}

/// The forecast of `kpi` over `horizon` steps from its readings in `series`; `None` when it has
/// too little history to forecast.
pub fn forecast_of(series: &[Series], kpi: &str, horizon: usize) -> Option<(f64, Json)> {
    let found = series.iter().find(|s| s.id == kpi)?;
    let result = analyse_series(found, Some(horizon));
    if result.status != Status::Ok || result.forecast.is_empty() {
        return None;
    }
    Some((result.step?, json!(result.forecast)))
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
            &json!({"id": caller.id, "name": caller.name, "roles": caller.roles}),
        ),
        None => Response::json(200, &Json::Null),
    }
}

fn published(request: &Request, _: &Params) -> Response {
    let kpi = match kpi_urn(request.param("kpi").unwrap_or_default()) {
        Ok(kpi) => kpi,
        Err(why) => return bad(&why),
    };
    match sql::query(
        "select id, days, horizon, note, step, points, publisher_name as \"publisherName\", \
         created_at as \"publishedAt\" from published where kpi = $1 order by id desc limit $2",
        &[Value::from(kpi), Value::Int(PUBLISHED_LISTED)],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

fn publish(request: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    if let Err(answer) = caller.require_role(&[STEWARD]) {
        return answer;
    }
    let wanted: Publish = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    let (note, horizon) = match checked_publish(&wanted) {
        Ok(checked) => checked,
        Err(why) => return bad(&why),
    };
    // One indicator's history: one page, inside a request's time.
    let series = match read_pages(
        now() - wanted.days * DAY,
        Some(&wanted.kpi),
        gateway::get_json,
        || false,
    ) {
        Ok(series) => series,
        Err(failure) => return failure.response(),
    };
    let Some((step, points)) = forecast_of(&series, &wanted.kpi, horizon) else {
        return Response::problem(
            409,
            "Conflict",
            "this indicator has too little history over the period to forecast",
        );
    };
    match PUBLISHED.insert(
        &caller,
        &[
            "kpi",
            "days",
            "horizon",
            "note",
            "step",
            "points",
            "publisher_name",
        ],
        vec![
            Value::from(wanted.kpi.as_str()),
            Value::from(wanted.days),
            Value::from(wanted.horizon),
            Value::from(note.as_str()),
            Value::Float(step),
            Value::Json(points.to_string()),
            Value::from(caller.name.as_str()),
        ],
        &format!("published a forecast: {note}"),
    ) {
        Ok(rows) => {
            let row = sql::objects(&rows).into_iter().next().unwrap_or_default();
            Response::json(
                201,
                &json!({"id": row.get("id"), "days": wanted.days, "horizon": wanted.horizon, "note": note, "step": step, "points": points, "publisherName": caller.name, "publishedAt": row.get("created_at")}),
            )
        }
        Err(answer) => answer,
    }
}

fn withdraw(_: &Request, params: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    if let Err(answer) = caller.require_role(&[STEWARD]) {
        return answer;
    }
    let id = match id(params, "published forecast") {
        Ok(id) => id,
        Err(answer) => return answer,
    };
    match PUBLISHED.delete(&caller, id, "withdrew a published forecast") {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

fn follows(_: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    match sql::query(
        "select id, kpi from follows where owner_id = $1 order by id desc",
        &[Value::from(caller.id.as_str())],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Follow {
    kpi: String,
}

fn follow(request: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let wanted: Follow = match request.json() {
        Ok(wanted) => wanted,
        Err(answer) => return answer,
    };
    if let Err(why) = kpi_urn(&wanted.kpi) {
        return bad(&why);
    }
    match sql::query(
        "select count(*) as n, count(*) filter (where kpi = $2) as same from follows where owner_id = $1",
        &[Value::from(caller.id.as_str()), Value::from(wanted.kpi.as_str())],
    ) {
        Ok(rows) => {
            let row = sql::objects(&rows).into_iter().next().unwrap_or_default();
            let count = |key: &str| row.get(key).and_then(Json::as_i64).unwrap_or(0);
            if count("same") > 0 {
                return Response::problem(409, "Conflict", "you follow this indicator already");
            }
            if count("n") >= MOST_FOLLOWS {
                return Response::problem(
                    409,
                    "Conflict",
                    &format!("at most {MOST_FOLLOWS} indicators are followed; stop following one first"),
                );
            }
        }
        Err(err) => return Response::from_sql(err),
    }
    match FOLLOWS.insert(
        &caller,
        &["kpi"],
        vec![Value::from(wanted.kpi.as_str())],
        "followed an indicator",
    ) {
        Ok(rows) => {
            let row = sql::objects(&rows).into_iter().next().unwrap_or_default();
            Response::json(201, &json!({"id": row.get("id"), "kpi": wanted.kpi}))
        }
        Err(answer) => answer,
    }
}

fn unfollow(_: &Request, params: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let id = match id(params, "follow") {
        Ok(id) => id,
        Err(answer) => return answer,
    };
    match FOLLOWS.delete(&caller, id, "stopped following an indicator") {
        Ok(()) => Response::no_content(),
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
        .post("/api/forecasts", record)
        .get("/api/forecasts", list)
        .post("/api/reports", monthly)
        .get("/api/me", me)
        .get("/api/published", published)
        .post("/api/published", publish)
        .delete("/api/published/{id}", withdraw)
        .get("/api/follows", follows)
        .post("/api/follows", follow)
        .delete("/api/follows/{id}", unfollow)
        .get("/api/history", changes)
        .handle(&request)
}

jc_app_sdk::app!(handle);

// The job's export, a root-level `record-forecasts: func() -> result<_, string>` beside the
// request handler, as `wit/server.wit` declares it (AP-159).
#[cfg(target_arch = "wasm32")]
mod job {
    wit_bindgen::generate!({ path: "wit", world: "helsinki:kpi-forecast/server", generate_all });

    struct Jobs;

    impl Guest for Jobs {
        fn record_forecasts() -> Result<(), String> {
            super::record_forecasts()
        }
    }

    export!(Jobs);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(horizon: i64, note: &str) -> Publish {
        Publish {
            kpi: "urn:ngsi-ld:KeyPerformanceIndicator:helsinki:bikes-in-use".into(),
            days: 30,
            horizon,
            note: note.into(),
        }
    }

    /// T-3827: a steward publishes a forecast of 1 to 48 steps with a note on one line, of an
    /// indicator and a period the page offers.
    #[test]
    fn a_publication_has_a_horizon_and_a_note_on_one_line() {
        assert_eq!(
            checked_publish(&wanted(12, "  for the board  ")),
            Ok(("for the board".to_owned(), 12))
        );
        assert_eq!(
            checked_publish(&wanted(MOST_HORIZON, "x")).map(|c| c.1),
            Ok(48)
        );
        for horizon in [0, -1, MOST_HORIZON + 1] {
            assert_eq!(
                checked_publish(&wanted(horizon, "x")),
                Err("the horizon is 1 to 48 steps".to_owned())
            );
        }
        for note in ["", "   ", "two\nlines", &"x".repeat(201)] {
            assert_eq!(
                checked_publish(&wanted(6, note)),
                Err("a note is 1 to 200 characters, on one line".to_owned())
            );
        }
        assert!(checked_publish(&Publish {
            days: 14,
            ..wanted(6, "x")
        })
        .is_err());
        assert!(checked_publish(&Publish {
            kpi: "urn:ngsi-ld:Building:x".into(),
            ..wanted(6, "x")
        })
        .is_err());
    }

    /// The published forecast has the horizon asked for; an indicator with too little history,
    /// or one not read, has none.
    #[test]
    fn a_published_forecast_has_the_horizon_asked_for() {
        let day = DAY as f64;
        let points: Vec<Point> = (0..30)
            .map(|i| Point {
                t: f64::from(i) * day,
                v: 100.0 + f64::from(i),
            })
            .collect();
        let series = vec![
            Series {
                id: "urn:ngsi-ld:KeyPerformanceIndicator:a".into(),
                points,
            },
            Series {
                id: "urn:ngsi-ld:KeyPerformanceIndicator:b".into(),
                points: vec![Point { t: 0.0, v: 1.0 }],
            },
        ];
        let (step, forecast) =
            forecast_of(&series, "urn:ngsi-ld:KeyPerformanceIndicator:a", 5).unwrap();
        assert!((step - day).abs() < 1.0);
        assert_eq!(forecast.as_array().map(Vec::len), Some(5));
        assert!(forecast_of(&series, "urn:ngsi-ld:KeyPerformanceIndicator:b", 5).is_none());
        assert!(forecast_of(&series, "urn:ngsi-ld:KeyPerformanceIndicator:c", 5).is_none());
    }

    /// AP-155 (T-3372): a failed job run ends with a sentence naming the period and what said no,
    /// never a statement's text or a body.
    #[test]
    fn a_job_failure_says_which_period_and_why() {
        let quota = Failure::Sql(sql::Error::Quota("the App's 100 MiB are used".into()));
        assert_eq!(
            quota.sentence(30),
            "the 30-day forecasts were not recorded: the App's database said: the App's 100 MiB are used"
        );
        assert_eq!(
            Failure::Sql(sql::Error::Timeout).sentence(7),
            "the 7-day forecasts were not recorded: the App's database took too long"
        );
        assert_eq!(
            PERIODS,
            [7, 30, 90],
            "the job records every period the page offers"
        );
        for days in PERIODS {
            assert_eq!(period(days), Ok(days));
        }
    }

    const KPI: &str = "urn:ngsi-ld:KeyPerformanceIndicator:helsinki:bikes-in-use";

    fn entity(i: usize) -> Json {
        json!({"id": format!("urn:ngsi-ld:KeyPerformanceIndicator:k{i}"), "currentValue": {"values": [[1.0, "2026-10-01T10:00:00Z"]]}})
    }

    /// T-6197: the history is read page by page from the App's Endpoint with `after`, until a page
    /// comes back short; one indicator alone when named; a later page the time no longer allows is
    /// not asked for, and the job says how far it came.
    #[test]
    fn the_history_is_read_page_by_page_until_a_short_page() {
        let from = instant("2026-10-01T00:00:00Z").expect("t");
        let mut asked = Vec::new();
        let series = read_pages(
            from,
            None,
            |path| {
                asked.push(path.to_owned());
                let n = if asked.len() < 3 { PAGE } else { 7 };
                Ok((0..n).map(entity).collect())
            },
            || true,
        )
        .unwrap_or_default();
        assert_eq!(series.len(), 2 * PAGE + 7);
        assert_eq!(asked.len(), 3);
        assert_eq!(
            asked[2],
            "/ngsi-ld/v1/temporal/entities?type=KeyPerformanceIndicator&attrs=currentValue&timerel=after&timeAt=2026-10-01T00%3A00%3A00Z&lastN=2000&options=temporalValues&limit=100&offset=200"
        );
        assert!(asked.iter().all(|p| !p.contains("between")));

        assert!(history_path(from, Some(KPI), 0)
            .contains("&id=urn%3Angsi-ld%3AKeyPerformanceIndicator%3Ahelsinki%3Abikes-in-use&"));

        let mut pages = 0;
        let late = read_pages(
            from,
            None,
            |_| {
                pages += 1;
                Ok((0..PAGE).map(entity).collect())
            },
            || false,
        );
        assert_eq!(pages, 1);
        let Err(failure) = late else {
            panic!("the read went on past its time")
        };
        assert_eq!(
            failure.why(),
            "the indicators' history took longer than the job may wait, after 100 indicators"
        );

        let refused = read_pages(
            from,
            None,
            |_| Err(gateway::Error::Status(504, String::new())),
            || true,
        );
        assert!(matches!(
            refused,
            Err(Failure::Gateway(gateway::Error::Status(504, _)))
        ));

        let mut endless = 0;
        let bounded = read_pages(
            from,
            None,
            |_| {
                endless += 1;
                Ok((0..PAGE).map(entity).collect())
            },
            || true,
        );
        assert_eq!(endless, MOST_PAGES);
        assert_eq!(
            bounded.map(|s| s.len()).unwrap_or_default(),
            MOST_PAGES * PAGE
        );
    }

    /// Each period's forecast is made from the readings of the period only.
    #[test]
    fn a_period_keeps_the_readings_from_its_start() {
        let series = vec![Series {
            id: KPI.into(),
            points: (0..10)
                .map(|i| Point {
                    t: f64::from(i) * DAY as f64,
                    v: f64::from(i),
                })
                .collect(),
        }];
        let kept = since(&series, 7 * DAY);
        assert_eq!(kept[0].id, KPI);
        assert_eq!(
            kept[0].points.iter().map(|p| p.v).collect::<Vec<_>>(),
            vec![7.0, 8.0, 9.0]
        );
    }

    /// The job writes this month's report and last month's, across a year's end too.
    #[test]
    fn the_job_writes_this_months_report_and_the_last_ones() {
        let at = |text: &str| instant(text).expect("t");
        assert_eq!(
            report_months(at("2026-10-31T03:20:00Z")),
            ["2026-10", "2026-09"]
        );
        assert_eq!(
            report_months(at("2027-01-01T03:20:00Z")),
            ["2027-01", "2026-12"]
        );
        assert_eq!(report_key("2026-10"), "reports/2026-10.csv");
    }

    #[test]
    fn an_indicator_is_a_kpi_urn_and_a_period_one_the_page_offers() {
        assert_eq!(kpi_urn(KPI), Ok(KPI));
        for wrong in [
            "",
            "urn:ngsi-ld:KeyPerformanceIndicator:",
            "urn:ngsi-ld:Vehicle:1",
            "urn:ngsi-ld:KeyPerformanceIndicator:a,b",
            "urn:ngsi-ld:KeyPerformanceIndicator:a b",
        ] {
            assert!(kpi_urn(wrong).is_err(), "{wrong}");
        }
        assert_eq!(period(30), Ok(30));
        for wrong in [0, 1, 14, 365, -7] {
            assert!(period(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_month_runs_from_its_first_day_to_the_next_months() {
        let (start, end) = month_bounds("2026-02").expect("a month");
        assert_eq!(iso(start), "2026-02-01T00:00:00Z");
        assert_eq!(iso(end), "2026-03-01T00:00:00Z");
        assert_eq!(
            iso(month_bounds("2026-12").expect("december").1),
            "2027-01-01T00:00:00Z"
        );
        for wrong in [
            "",
            "2026",
            "2026-13",
            "2026-00",
            "26-01",
            "2026-1",
            "2026-01-01",
            "abcd-ef",
        ] {
            assert!(month_bounds(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn readings_are_the_current_values_with_a_time() {
        let entities = vec![
            json!({"id": KPI, "currentValue": {"type": "Property", "values": [[12.5, "2026-10-01T10:00:00Z"], ["n/a", "2026-10-01T11:00:00Z"], [13, "bad"]]}}),
            json!({"id": "urn:ngsi-ld:KeyPerformanceIndicator:x", "currentValue": [{"value": 4, "observedAt": "2026-10-01T10:00:00+03:00"}]}),
            json!({"id": "urn:ngsi-ld:KeyPerformanceIndicator:empty"}),
            json!({"currentValue": []}),
        ];
        let series = series_of(&entities);
        assert_eq!(series.len(), 3);
        assert_eq!(series[0].points.len(), 1);
        assert_eq!(series[0].points[0].v, 12.5);
        assert_eq!(
            series[1].points[0].t,
            instant("2026-10-01T07:00:00Z").expect("t") as f64
        );
        assert!(series[2].points.is_empty());
    }

    #[test]
    fn a_forecast_is_kept_for_an_indicator_with_enough_history_only() {
        let t0 = instant("2026-09-01T00:00:00Z").expect("t") as f64;
        let long = Series {
            id: KPI.into(),
            points: (0..60)
                .map(|i| Point {
                    t: t0 + f64::from(i) * DAY as f64,
                    v: 100.0 + f64::from(i) + f64::from(i % 7),
                })
                .collect(),
        };
        let short = Series {
            id: "urn:ngsi-ld:KeyPerformanceIndicator:short".into(),
            points: vec![Point { t: t0, v: 1.0 }],
        };
        let rows = forecasts_of(&[long, short]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["kpi"], KPI);
        assert_eq!(rows[0]["step"], DAY as f64);
        assert_eq!(rows[0]["direction"], "up");
        assert!(rows[0]["points"]
            .as_array()
            .is_some_and(|p| !p.is_empty() && p[0]["lo"].as_f64() <= p[0]["v"].as_f64()));
    }

    #[test]
    fn the_report_sets_each_forecast_point_of_the_month_against_its_reading() {
        let t = |s: &str| instant(s).expect("t") as f64;
        let forecasts = vec![
            json!({"kpi": KPI, "madeOn": "2026-09-30", "days": 30, "step": DAY as f64, "points": [
                {"t": t("2026-09-30T12:00:00Z"), "v": 10.0, "lo": 8.0, "hi": 12.0},
                {"t": t("2026-10-01T12:00:00Z"), "v": 11.0, "lo": 9.0, "hi": 13.0},
                {"t": t("2026-10-02T12:00:00Z"), "v": 12.0, "lo": 10.0, "hi": 14.0},
                {"t": t("2026-10-03T12:00:00Z"), "v": 13.0, "lo": 11.0, "hi": 15.0}
            ]}),
        ];
        let readings = vec![Series {
            id: KPI.into(),
            points: vec![
                Point {
                    t: t("2026-10-01T13:00:00Z"),
                    v: 12.0,
                },
                Point {
                    t: t("2026-10-02T12:00:00Z"),
                    v: 20.0,
                },
            ],
        }];
        let text = report("2026-10", &forecasts, &readings);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[1],
            "kpi,made_on,period_days,for_utc,expected,low_95,high_95,measured,within_95"
        );
        assert_eq!(lines.len(), 5, "{text}");
        assert_eq!(
            lines[2],
            format!("{KPI},2026-09-30,30,2026-10-01T12:00:00Z,11,9,13,12,yes")
        );
        assert_eq!(
            lines[3],
            format!("{KPI},2026-09-30,30,2026-10-02T12:00:00Z,12,10,14,20,no")
        );
        assert_eq!(
            lines[4],
            format!("{KPI},2026-09-30,30,2026-10-03T12:00:00Z,13,11,15,,")
        );
        assert_eq!(report("2026-10", &[], &[]).lines().count(), 2);
    }

    #[test]
    fn a_report_cell_is_never_a_formula_but_a_negative_number_stays_one() {
        assert_eq!(cell("-5"), "-5");
        assert_eq!(cell("=SUM(A1)"), "'=SUM(A1)");
        assert_eq!(cell("@x"), "'@x");
        assert_eq!(cell("a,b"), "\"a,b\"");
        assert_eq!(measured_at(None, 0.0, 1.0), None);
    }
}
