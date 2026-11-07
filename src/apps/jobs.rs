//! `GET /apps/{name}/api/services/jobs`: an App's scheduled jobs with their next and last run
//! (AP-154, AP-162, API/06 §3, T-3584). The schedules are `spec.server.jobs[]`, the last runs the
//! reconciler's `status.jobs[]` as the App's shard reported them; a run's body or credential is
//! never in either.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use jc_core::kinds::AppService;
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

use super::services;
use crate::state::AppState;

/// How far `nextRun` looks: a schedule that names no minute in a year (`0 0 31 2 *`) has none.
const HORIZON_MINUTES: i64 = 366 * 24 * 60;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    name: String,
    schedule: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_run: Option<LastRun>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct LastRun {
    at: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// The first minute after `now` that `schedule` names, in UTC, within a year.
// ponytail: a minute-by-minute scan, at most 527k cheap checks per job and ten jobs per App; a
// field-wise successor when a page lists many Apps' jobs.
pub fn next_run(schedule: &str, now: OffsetDateTime) -> Option<OffsetDateTime> {
    let start = now.replace_second(0).ok()?.replace_nanosecond(0).ok()? + Duration::MINUTE;
    (0..HORIZON_MINUTES)
        .map(|minute| start + Duration::minutes(minute))
        .find(|at| {
            jc_core::cron::matches(
                schedule,
                u32::from(at.minute()),
                u32::from(at.hour()),
                u32::from(at.day()),
                u32::from(u8::from(at.month())),
                u32::from(at.weekday().number_days_from_sunday()),
            )
            .unwrap_or(false)
        })
}

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(name): Path<String>,
) -> Response {
    let (project, spec, _) = match services::gate(
        &state,
        &headers,
        &uri,
        &name,
        (AppService::Jobs, &Method::GET),
    )
    .await
    {
        Ok(gated) => gated,
        Err(refused) => return *refused,
    };
    let runs = state
        .mirror
        .get(&project, "App", &name)
        .and_then(|envelope| envelope.status)
        .map(|status| status.jobs)
        .unwrap_or_default();
    let now = OffsetDateTime::now_utc();
    let jobs: Vec<JobStatus> = spec
        .server
        .iter()
        .flat_map(|server| &server.jobs)
        .map(|job| JobStatus {
            name: job.name.clone(),
            schedule: job.schedule.clone(),
            next_run: next_run(&job.schedule, now).and_then(|at| at.format(&Rfc3339).ok()),
            last_run: runs
                .iter()
                .find(|run| run.name == job.name)
                .map(|run| LastRun {
                    at: run.last_run.clone(),
                    ok: run.outcome == "succeeded",
                    message: run.message.clone(),
                }),
        })
        .collect();
    Json(jobs).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn the_next_run_is_the_first_named_minute_after_now() {
        let now = datetime!(2026-10-10 21:30:42 UTC);
        assert_eq!(
            next_run("*/15 * * * *", now),
            Some(datetime!(2026-10-10 21:45 UTC))
        );
        assert_eq!(
            next_run("30 21 * * *", now),
            Some(datetime!(2026-10-11 21:30 UTC)),
            "not now itself"
        );
        // A Monday at 06:00: the 12th.
        assert_eq!(
            next_run("0 6 * * 1", now),
            Some(datetime!(2026-10-12 6:00 UTC))
        );
        assert_eq!(next_run("0 0 31 2 *", now), None, "no such day");
        assert_eq!(next_run("not cron", now), None);
    }
}
