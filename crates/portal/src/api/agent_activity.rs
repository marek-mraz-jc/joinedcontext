//! What the agents did (T-4851, AG-134…AG-136): the runs the caller may see, each with its tool
//! calls and their verdicts, the writes it made with their reasons, its refusals, tokens and
//! cost; and the three signals an administrator watches: a spike of refusals, a run near its
//! token cap and a write outside office hours.
//!
//! The rows are the ones `GET …/agent-runs` lists for the caller: their own runs, or every
//! person's for an approver of the project (AG-93). A step carries its tool, its target and the
//! problem text of a refusal, masked like a pipeline's errors; never its input or output, which
//! may hold what a person typed (MF-52 `transcriptRedaction`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Datelike, Duration, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use crate::agents::run::{AgentRun, AgentRunEvent, AgentRunStatus};
use crate::agents::store::{AgentStore, RunFilter, StoreError};
use crate::alerts::{Event, Signal};
use crate::auth::session::CurrentUser;
use crate::error::ApiError;
use crate::pipeline_outcomes::mask_text;
use crate::state::AppState;
use crate::store::Mirror;

/// Refusals of one agent within an hour that make a spike.
pub const REFUSAL_SPIKE: usize = 5;
/// The share of a run's token cap, in percent, past which it is flagged.
pub const SPEND_SHARE_PERCENT: u64 = 80;
/// Office hours, UTC, Monday to Friday: 06:00–18:00 is 07:00–19:00 in Central European winter
/// time. ponytail: one fixed window; an organization's own zone and hours when one asks.
pub const OFFICE_HOURS_UTC: (u32, u32) = (6, 18);
/// The newest runs one answer reads, across projects, before its filters page them.
const SCANNED: usize = 500;
/// The longest problem text or reason kept per step.
const TEXT_CHARS: usize = 400;

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ActivityQuery {
    /// Only this project's runs.
    pub project: Option<String>,
    /// Only the runs of this agent profile.
    pub profile: Option<String>,
    /// Only this run.
    pub run: Option<String>,
    /// Only the runs this person started.
    pub person: Option<String>,
    /// Only the runs that read an Endpoint of this Context Space.
    pub space: Option<String>,
    /// `refused`: runs with a refused step; `ok`: runs without one.
    pub verdict: Option<String>,
    /// RFC 3339; runs started at or after.
    pub since: Option<String>,
    /// RFC 3339; runs started before.
    pub until: Option<String>,
    /// 1–50, default 20.
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivity {
    pub runs: Vec<ActivityRun>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// The signals over the runs the filters select, newest first; one per agent or run.
    pub alerts: Vec<AgentAlert>,
    /// The values the person and space filters offer: those of the runs this caller sees.
    pub choices: ActivityChoices,
}

#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityChoices {
    /// The starters of the runs, sorted.
    pub people: Vec<String>,
    /// The Context Spaces the runs read, sorted.
    pub spaces: Vec<String>,
    /// The projects in which this caller may subscribe to an agent's alerts (`PUT /alerts`,
    /// scope `agent`): those they approve in, sorted (T-5301).
    pub alert_projects: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRun {
    pub id: String,
    pub project: String,
    pub profile: String,
    pub title: String,
    pub started_by: String,
    pub status: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    pub spaces: Vec<String>,
    pub tokens: i64,
    /// The sum of the costs the provider reported, in USD; none when no call reported one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub refusals: usize,
    pub writes: usize,
    pub steps: Vec<ActivityStep>,
    /// Whether this caller may stop it now (`POST …/agent-runs/{id}/cancel`).
    pub may_stop: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum StepVerdict {
    Ok,
    Refused,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityStep {
    pub at: String,
    pub tool: String,
    /// What the call named: an Endpoint, a resource, a page; never its arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub verdict: StepVerdict,
    /// The call changes something (its operation is not read-only).
    pub write: bool,
    /// The refusal's problem text, or the reason a write gave; masked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum AlertKind {
    RefusalSpike,
    SpendShare,
    OffHoursWrite,
}

#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentAlert {
    pub kind: AlertKind,
    pub project: String,
    pub profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The run's title as its card shows it, so an alert names the run a person recognises.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_title: Option<String>,
    /// Refusals in the hour, percent of the cap, or writes outside office hours.
    pub count: u64,
    pub at: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/agents/activity",
    summary = "Agent Activity",
    description = "The agent runs the caller may see (their own, every person's for an approver \
        of the project), newest first, each with its tool calls, writes, refusals, tokens and \
        cost, and the alerts over them. Step arguments and outputs are never included.",
    tag = "agents",
    params(ActivityQuery),
    responses(
        (status = 200, description = "One page of runs and the alerts", body = AgentActivity),
        (status = 400, description = "A filter or the cursor is malformed", body = crate::error::ProblemDetails),
        (status = 401, description = "Unauthorized", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller reads no project", body = crate::error::ProblemDetails)
    )
)]
pub async fn list_agent_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<ActivityQuery>,
) -> Result<Json<AgentActivity>, ApiError> {
    let filter = Filter::parse(&query)?;
    let answer = activity(&state, &user, &filter, Utc::now()).await?;
    tracing::info!(
        target: "audit",
        user = %user.0.identity.username,
        project = ?query.project, profile = ?query.profile, run = ?query.run,
        runs = answer.runs.len(),
        "agent activity read"
    );
    Ok(Json(answer))
}

const NO_PROJECT: &str = "Your roles read no project, so no agent run is yours to see. Ask a \
     steward of the project or an organization administrator to grant read on it.";

/// The filters, checked: a malformed time, verdict, limit or cursor is a `400`.
struct Filter<'a> {
    query: &'a ActivityQuery,
    refused: Option<bool>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    limit: usize,
    after: Option<(DateTime<Utc>, String)>,
}

impl<'a> Filter<'a> {
    fn parse(query: &'a ActivityQuery) -> Result<Self, ApiError> {
        let time = |name: &str, text: &Option<String>| {
            text.as_deref()
                .map(|t| {
                    parse_time(t).ok_or_else(|| {
                        ApiError::BadRequest(format!("{name}: '{t}' is not an RFC 3339 time"))
                    })
                })
                .transpose()
        };
        let refused = match query.verdict.as_deref() {
            None => None,
            Some("refused") => Some(true),
            Some("ok") => Some(false),
            Some(other) => {
                return Err(ApiError::BadRequest(format!(
                    "verdict: '{other}' is neither 'ok' nor 'refused'"
                )))
            }
        };
        let limit = query.limit.unwrap_or(20);
        if !(1..=50).contains(&limit) {
            return Err(ApiError::BadRequest("limit: 1 to 50".into()));
        }
        let after = query.cursor.as_deref().map(parse_cursor).transpose()?;
        Ok(Self {
            query,
            refused,
            since: time("since", &query.since)?,
            until: time("until", &query.until)?,
            limit,
            after,
        })
    }

    /// The agent or run the tab is about; the filter choices are drawn from these runs.
    fn in_scope(&self, run: &AgentRun) -> bool {
        let q = self.query;
        q.profile.as_ref().is_none_or(|p| &run.profile == p)
            && q.run.as_ref().is_none_or(|id| &run.id == id)
    }

    /// A person or space outside the choices names a run the caller does not see: `403`
    /// naming the field, so a typed or stale value never reads as an empty history.
    fn check_choices(&self, choices: &ActivityChoices) -> Result<(), ApiError> {
        let outside = |name: &str, value: &Option<String>, offered: &[String]| match value {
            Some(v) if !offered.contains(v) => Err(ApiError::Denied(format!(
                "{name}: '{v}' is in no agent run you may see; pick one from the list"
            ))),
            _ => Ok(()),
        };
        outside("person", &self.query.person, &choices.people)?;
        outside("space", &self.query.space, &choices.spaces)
    }

    fn admits(&self, run: &AgentRun) -> bool {
        let q = self.query;
        let created = parse_time(&run.created_at);
        self.in_scope(run)
            && q.person.as_ref().is_none_or(|p| &run.created_by == p)
            && q.space.as_ref().is_none_or(|s| spaces_of(run).contains(s))
            && self
                .since
                .is_none_or(|since| created.is_some_and(|c| c >= since))
            && self
                .until
                .is_none_or(|until| created.is_some_and(|c| c < until))
    }
}

fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn parse_cursor(text: &str) -> Result<(DateTime<Utc>, String), ApiError> {
    text.split_once('|')
        .and_then(|(at, id)| Some((parse_time(at)?, id.to_owned())))
        .filter(|(_, id)| !id.is_empty())
        .ok_or_else(|| ApiError::BadRequest("cursor: not a cursor this route gave".into()))
}

/// One project's runs the caller sees, with whether they may stop each.
struct Seen {
    run: AgentRun,
    may_stop: bool,
}

async fn visible(
    state: &AppState,
    user: &CurrentUser,
    filter: &Filter<'_>,
) -> Result<(Vec<Seen>, ActivityChoices), ApiError> {
    let identity = &user.0.identity;
    let mut seen = Vec::new();
    let (mut people, mut spaces) = (BTreeSet::new(), BTreeSet::new());
    let mut alert_projects = Vec::new();
    let mut reads_any = false;
    for project in state.mirror.namespaces() {
        if filter.query.project.as_ref().is_some_and(|p| p != &project) {
            continue;
        }
        let effective = crate::permissions::for_request(state, identity, &project);
        if !effective.may_read_project() {
            continue;
        }
        reads_any = true;
        if crate::api::changes::may_approve_anything(state, identity, &project).is_ok() {
            alert_projects.push(project.clone());
        }
        let proposes = effective
            .check("App", jc_core::kinds::Verb::Propose, None)
            .is_ok();
        for run in crate::api::agent_registry::visible_runs(state, identity, &project).await? {
            if filter.in_scope(&run) {
                people.insert(run.created_by.clone());
                spaces.extend(spaces_of(&run));
            }
            if filter.admits(&run) {
                let live = AgentRunStatus::parse(&run.status).is_some_and(|s| !s.is_terminal());
                // `visible_runs` holds only the caller's runs unless they approve in the project,
                // which is the rule the cancel route applies (AG-93).
                seen.push(Seen {
                    may_stop: live && proposes,
                    run,
                });
            }
        }
    }
    if !reads_any {
        return Err(ApiError::Denied(NO_PROJECT.into()));
    }
    seen.sort_by_key(|s| std::cmp::Reverse(order_key(&s.run)));
    seen.truncate(SCANNED);
    let choices = ActivityChoices {
        people: people.into_iter().collect(),
        spaces: spaces.into_iter().collect(),
        alert_projects,
    };
    filter.check_choices(&choices)?;
    Ok((seen, choices))
}

fn order_key(run: &AgentRun) -> (Option<DateTime<Utc>>, String) {
    (parse_time(&run.created_at), run.id.clone())
}

async fn activity(
    state: &AppState,
    user: &CurrentUser,
    filter: &Filter<'_>,
    now: DateTime<Utc>,
) -> Result<AgentActivity, ApiError> {
    let (seen, choices) = visible(state, user, filter).await?;
    let ids: Vec<String> = seen.iter().map(|s| s.run.id.clone()).collect();
    let mut events = state
        .agents
        .events_of(&ids, &["tool", "usage"])
        .await
        .map_err(crate::api::agent_runs::unavailable)?;
    let rows: Vec<ActivityRun> = seen
        .into_iter()
        .map(|s| {
            let own = events.remove(&s.run.id).unwrap_or_default();
            row(s, &own)
        })
        .collect();
    let alerts = alerts(&rows, &|profile| token_cap(&state.mirror, profile), now);
    Ok(AgentActivity {
        choices,
        ..page(rows, filter, alerts)
    })
}

/// The page after the cursor, the verdict filter applied, and the cursor of the next one.
fn page(rows: Vec<ActivityRun>, filter: &Filter<'_>, alerts: Vec<AgentAlert>) -> AgentActivity {
    let mut runs: Vec<ActivityRun> = rows
        .into_iter()
        .filter(|r| {
            filter.after.as_ref().is_none_or(|(at, id)| {
                (parse_time(&r.created_at), r.id.clone()) < (Some(*at), id.clone())
            })
        })
        .filter(|r| {
            filter
                .refused
                .is_none_or(|refused| refused == (r.refusals > 0))
        })
        .take(filter.limit + 1)
        .collect();
    let next_cursor = if runs.len() > filter.limit {
        runs.truncate(filter.limit);
        runs.last().map(|r| format!("{}|{}", r.created_at, r.id))
    } else {
        None
    };
    AgentActivity {
        runs,
        next_cursor,
        alerts,
        choices: ActivityChoices::default(),
    }
}

fn token_cap(mirror: &Mirror, profile: &str) -> Option<u64> {
    mirror
        .get(crate::permissions::ORG_NAMESPACE, "AgentProfile", profile)
        .and_then(|p| p.spec.pointer("/model/maxTokensPerRun")?.as_u64())
}

fn spaces_of(run: &AgentRun) -> Vec<String> {
    let mut spaces: Vec<String> = run
        .endpoints
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["space"].as_str().map(str::to_owned))
        .collect();
    spaces.sort();
    spaces.dedup();
    spaces
}

fn row(seen: Seen, events: &[AgentRunEvent]) -> ActivityRun {
    let run = seen.run;
    let steps: Vec<ActivityStep> = events
        .iter()
        .filter(|e| e.kind == "tool")
        .map(step)
        .collect();
    let costs: Vec<f64> = events
        .iter()
        .filter(|e| e.kind == "usage")
        .filter_map(|e| e.payload["costUsd"].as_f64())
        .filter(|c| c.is_finite() && *c >= 0.0)
        .collect();
    let duration_ms = run
        .started_at
        .as_deref()
        .or(Some(&run.created_at))
        .and_then(parse_time)
        .zip(run.finished_at.as_deref().and_then(parse_time))
        .map(|(from, to)| (to - from).num_milliseconds());
    ActivityRun {
        spaces: spaces_of(&run),
        title: run.title.clone().unwrap_or_else(|| run.app_name.clone()),
        refusals: steps
            .iter()
            .filter(|s| s.verdict == StepVerdict::Refused)
            .count(),
        writes: steps
            .iter()
            .filter(|s| s.write && s.verdict == StepVerdict::Ok)
            .count(),
        cost_usd: (!costs.is_empty()).then(|| (costs.iter().sum::<f64>() * 1e6).round() / 1e6),
        steps,
        duration_ms,
        may_stop: seen.may_stop,
        id: run.id,
        project: run.project,
        profile: run.profile,
        started_by: run.created_by,
        status: run.status,
        created_at: run.created_at,
        finished_at: run.finished_at,
        tokens: run.tokens_used,
    }
}

/// One `tool` event as a step: tool, target, verdict, and the problem text or the write's
/// reason, masked; never the arguments or the output.
fn step(event: &AgentRunEvent) -> ActivityStep {
    let p = &event.payload;
    let tool = p["tool"].as_str().unwrap_or("unknown").to_owned();
    let refused = p["status"] == "failed" || p["exitCode"].as_i64().is_some_and(|c| c != 0);
    let write = crate::ops::find(&tool).is_some_and(|op| !op.annotations.read_only_hint);
    let text = if refused {
        p["error"].as_str().map(str::to_owned).or_else(|| {
            p["refused"]
                .as_array()
                .map(|r| format!("{} block(s) refused", r.len()))
        })
    } else if write {
        p["input"]["reason"].as_str().map(str::to_owned)
    } else {
        None
    };
    ActivityStep {
        at: event.created_at.clone(),
        target: target(&p["input"]),
        verdict: if refused {
            StepVerdict::Refused
        } else {
            StepVerdict::Ok
        },
        write,
        reason: text.map(|t| clip(&mask_text(&t))),
        duration_ms: p["durationMs"].as_u64(),
        tool,
    }
}

/// The name a call acted on: the first of its naming arguments that is a short string.
fn target(input: &Value) -> Option<String> {
    [
        "endpoint", "name", "kind", "page", "space", "pipeline", "project",
    ]
    .iter()
    .filter_map(|key| input[*key].as_str())
    .find(|text| !text.is_empty() && text.chars().count() <= 120)
    .map(mask_text)
}

fn clip(text: &str) -> String {
    match text.char_indices().nth(TEXT_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

/// Whether a moment falls outside office hours (`OFFICE_HOURS_UTC`, Monday to Friday).
pub fn off_hours(at: DateTime<Utc>) -> bool {
    let (from, to) = OFFICE_HOURS_UTC;
    matches!(at.weekday(), Weekday::Sat | Weekday::Sun) || at.hour() < from || at.hour() >= to
}

/// The alerts over the rows: one refusal spike per agent profile with `REFUSAL_SPIKE` or more
/// refusals in the hour before `now`, one per run past `SPEND_SHARE_PERCENT` of its profile's
/// token cap, and one per run that wrote outside office hours. Newest first.
fn alerts(
    rows: &[ActivityRun],
    cap: &dyn Fn(&str) -> Option<u64>,
    now: DateTime<Utc>,
) -> Vec<AgentAlert> {
    let hour_ago = now - Duration::hours(1);
    let mut spikes: BTreeMap<(String, String), (u64, String)> = BTreeMap::new();
    let mut found = Vec::new();
    let mut caps: HashMap<String, Option<u64>> = HashMap::new();
    for run in rows {
        for refusal in run
            .steps
            .iter()
            .filter(|s| s.verdict == StepVerdict::Refused)
        {
            if parse_time(&refusal.at).is_some_and(|at| at >= hour_ago && at <= now) {
                let entry = spikes
                    .entry((run.project.clone(), run.profile.clone()))
                    .or_default();
                entry.0 += 1;
                entry.1 = entry.1.clone().max(refusal.at.clone());
            }
        }
        let cap = *caps
            .entry(run.profile.clone())
            .or_insert_with(|| cap(&run.profile));
        let used = u64::try_from(run.tokens).unwrap_or(0);
        if let Some(cap) = cap.filter(|c| *c > 0 && used * 100 >= c * SPEND_SHARE_PERCENT) {
            found.push(alert(
                AlertKind::SpendShare,
                run,
                used * 100 / cap,
                &run.created_at,
            ));
        }
        let late: Vec<&ActivityStep> = run
            .steps
            .iter()
            .filter(|s| s.write && s.verdict == StepVerdict::Ok)
            .filter(|s| parse_time(&s.at).is_some_and(off_hours))
            .collect();
        if let Some(first) = late.first() {
            found.push(alert(
                AlertKind::OffHoursWrite,
                run,
                late.len() as u64,
                &first.at,
            ));
        }
    }
    for ((project, profile), (count, at)) in spikes {
        if count >= REFUSAL_SPIKE as u64 {
            found.push(AgentAlert {
                kind: AlertKind::RefusalSpike,
                project,
                profile,
                run_id: None,
                run_title: None,
                count,
                at,
            });
        }
    }
    found.sort_by_key(|a| std::cmp::Reverse(a.at.clone()));
    found
}

fn alert(kind: AlertKind, run: &ActivityRun, count: u64, at: &str) -> AgentAlert {
    AgentAlert {
        kind,
        project: run.project.clone(),
        profile: run.profile.clone(),
        run_id: Some(run.id.clone()),
        run_title: Some(run.title.clone()),
        count,
        at: at.to_owned(),
    }
}

/// The alerts of the last hour as incident signals for the subscribers of each profile
/// (T-5301): every person-started run of every project, as an approver sees them, a run counted
/// while it is live or finished within the hour. One signal per project, profile and kind, the
/// newest; its detail names the run and the count.
pub async fn signals(
    store: &AgentStore,
    mirror: &Mirror,
    now: DateTime<Utc>,
) -> Result<Vec<Signal>, StoreError> {
    let hour_ago = now - Duration::hours(1);
    let filter = RunFilter {
        origin: Some("person".into()),
        ..RunFilter::default()
    };
    let mut seen = Vec::new();
    for project in mirror.namespaces() {
        for run in store
            .list_runs_filtered(&project, &filter, SCANNED as i64)
            .await?
        {
            let live = AgentRunStatus::parse(&run.status).is_some_and(|s| !s.is_terminal());
            let recent = run
                .finished_at
                .as_deref()
                .and_then(parse_time)
                .is_some_and(|at| at >= hour_ago);
            if live || recent {
                seen.push(Seen {
                    run,
                    may_stop: false,
                });
            }
        }
    }
    let ids: Vec<String> = seen.iter().map(|s| s.run.id.clone()).collect();
    let mut events = store.events_of(&ids, &["tool", "usage"]).await?;
    let rows: Vec<ActivityRun> = seen
        .into_iter()
        .map(|s| {
            let own = events.remove(&s.run.id).unwrap_or_default();
            row(s, &own)
        })
        .collect();
    let mut found: Vec<Signal> = Vec::new();
    for alert in alerts(&rows, &|profile| token_cap(mirror, profile), now) {
        let event = match alert.kind {
            AlertKind::RefusalSpike => Event::RefusalSpike,
            AlertKind::SpendShare => Event::SpendShare,
            AlertKind::OffHoursWrite => Event::OffHoursWrite,
        };
        let key = (alert.project.clone(), alert.profile.clone(), event);
        if !found.iter().any(|signal| signal.key == key) {
            found.push(Signal {
                key,
                detail: signal_detail(&alert),
            });
        }
    }
    Ok(found)
}

/// What a notice says beside its title: the count, and the run it names.
fn signal_detail(alert: &AgentAlert) -> String {
    let run = alert.run_title.as_deref().unwrap_or_default();
    match alert.kind {
        AlertKind::RefusalSpike => format!("{} refused tool calls in the last hour", alert.count),
        AlertKind::SpendShare => format!("run “{run}” used {} % of its token cap", alert.count),
        AlertKind::OffHoursWrite => format!(
            "run “{run}” wrote {} time(s) outside office hours, the first at {}",
            alert.count, alert.at
        ),
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/organization/agents/activity", get(list_agent_activity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> DateTime<Utc> {
        parse_time(text).expect("a time")
    }

    fn event(seq: i64, at: &str, payload: Value) -> AgentRunEvent {
        AgentRunEvent {
            run_id: "r1".into(),
            seq,
            kind: "tool".into(),
            payload,
            created_at: at.into(),
        }
    }

    fn row_with(id: &str, created: &str, tokens: i64, steps: Vec<ActivityStep>) -> ActivityRun {
        ActivityRun {
            id: id.into(),
            project: "helsinki".into(),
            profile: "builder".into(),
            title: "Air".into(),
            started_by: "ana@hel.fi".into(),
            status: "running".into(),
            created_at: created.into(),
            finished_at: None,
            duration_ms: None,
            spaces: vec![],
            tokens,
            cost_usd: None,
            refusals: steps
                .iter()
                .filter(|s| s.verdict == StepVerdict::Refused)
                .count(),
            writes: 0,
            steps,
            may_stop: false,
        }
    }

    fn refusal(at: &str) -> ActivityStep {
        step(&event(
            1,
            at,
            json!({ "tool": "jc_resource_list", "status": "failed", "error": "no" }),
        ))
    }

    #[test]
    fn a_step_keeps_tool_target_and_verdict_and_never_its_arguments_or_output() {
        let ok = step(&event(
            1,
            "2026-10-21T10:00:00Z",
            json!({ "tool": "jc_resource_list", "status": "ok", "durationMs": 12,
                    "input": { "kind": "Endpoint", "q": "PERSONAL-QUESTION" },
                    "output": { "items": ["SECRET-ROW"] } }),
        ));
        assert_eq!(ok.verdict, StepVerdict::Ok);
        assert_eq!(ok.target.as_deref(), Some("Endpoint"));
        assert!(!ok.write, "a list reads");
        assert_eq!(ok.reason, None);
        let text = serde_json::to_string(&ok).unwrap();
        assert!(!text.contains("PERSONAL-QUESTION") && !text.contains("SECRET-ROW"));
    }

    #[test]
    fn a_refusal_carries_its_problem_text_masked() {
        let refused = step(&event(
            1,
            "2026-10-21T10:00:00Z",
            json!({ "tool": "jc_endpoint_propose", "status": "failed",
                    "error": "forbidden: token Bearer abc.def.ghi rejected" }),
        ));
        assert_eq!(refused.verdict, StepVerdict::Refused);
        assert!(refused.write, "a proposal changes something");
        let reason = refused.reason.unwrap();
        assert!(reason.starts_with("forbidden"), "{reason}");
        assert!(!reason.contains("abc.def.ghi"), "{reason}");
    }

    #[test]
    fn a_write_shows_the_reason_it_gave() {
        let write = step(&event(
            1,
            "2026-10-21T10:00:00Z",
            json!({ "tool": "jc_resource_propose", "status": "ok",
                    "input": { "name": "air", "reason": "the sensor moved" } }),
        ));
        assert!(write.write);
        assert_eq!(write.reason.as_deref(), Some("the sensor moved"));
        assert_eq!(write.target.as_deref(), Some("air"));
    }

    #[test]
    fn office_hours_are_weekdays_six_to_eighteen_utc() {
        assert!(!off_hours(at("2026-10-21T06:00:00Z")), "Wednesday morning");
        assert!(off_hours(at("2026-10-21T05:59:59Z")));
        assert!(off_hours(at("2026-10-21T18:00:00Z")));
        assert!(off_hours(at("2026-10-24T10:00:00Z")), "Saturday");
    }

    #[test]
    fn each_alert_fires_once() {
        let now = at("2026-10-21T22:30:00Z");
        let spiky: Vec<ActivityStep> = (0..REFUSAL_SPIKE)
            .map(|i| refusal(&format!("2026-10-21T22:{:02}:00Z", i + 1)))
            .collect();
        let mut late = step(&event(
            1,
            "2026-10-21T22:10:00Z",
            json!({ "tool": "jc_resource_propose", "status": "ok", "input": { "name": "a" } }),
        ));
        late.write = true;
        let mut late2 = step(&event(
            2,
            "2026-10-21T22:20:00Z",
            json!({ "tool": "jc_resource_propose", "status": "ok", "input": { "name": "b" } }),
        ));
        late2.write = true;
        let rows = vec![
            row_with("r1", "2026-10-21T22:00:00Z", 10, spiky),
            row_with("r2", "2026-10-21T21:00:00Z", 900, vec![]),
            row_with("r3", "2026-10-21T20:00:00Z", 10, vec![late, late2]),
        ];
        let found = alerts(&rows, &|_| Some(1000), now);
        let kinds: Vec<AlertKind> = found.iter().map(|a| a.kind).collect();
        assert_eq!(kinds.len(), 3, "{found:?}");
        for kind in [
            AlertKind::RefusalSpike,
            AlertKind::SpendShare,
            AlertKind::OffHoursWrite,
        ] {
            assert_eq!(kinds.iter().filter(|k| **k == kind).count(), 1, "{kind:?}");
        }
        let spend = found
            .iter()
            .find(|a| a.kind == AlertKind::SpendShare)
            .unwrap();
        assert_eq!((spend.run_id.as_deref(), spend.count), (Some("r2"), 90));
        assert_eq!(spend.run_title.as_deref(), Some("Air"));
        let late = found
            .iter()
            .find(|a| a.kind == AlertKind::OffHoursWrite)
            .unwrap();
        assert_eq!((late.run_id.as_deref(), late.count), (Some("r3"), 2));
    }

    #[test]
    fn no_alert_below_the_thresholds_or_outside_the_hour() {
        let now = at("2026-10-21T12:00:00Z");
        let few: Vec<ActivityStep> = (0..REFUSAL_SPIKE - 1)
            .map(|_| refusal("2026-10-21T11:30:00Z"))
            .collect();
        let old: Vec<ActivityStep> = (0..REFUSAL_SPIKE)
            .map(|_| refusal("2026-10-21T10:30:00Z"))
            .collect();
        let rows = vec![
            row_with("r1", "2026-10-21T11:00:00Z", 799, few),
            row_with("r2", "2026-10-21T10:00:00Z", 0, old),
        ];
        assert_eq!(alerts(&rows, &|_| Some(1000), now), vec![]);
        assert_eq!(alerts(&rows, &|_| None, now), vec![], "no cap, no share");
    }

    #[test]
    fn a_page_ends_with_a_cursor_that_starts_the_next() {
        let query = ActivityQuery {
            limit: Some(2),
            ..ActivityQuery::default()
        };
        let rows = |n: usize| -> Vec<ActivityRun> {
            (0..n)
                .map(|i| {
                    row_with(
                        &format!("r{i}"),
                        &format!("2026-10-21T1{}:00:00Z", 9 - i),
                        0,
                        vec![],
                    )
                })
                .collect()
        };
        let first = page(rows(3), &Filter::parse(&query).unwrap(), vec![]);
        assert_eq!(first.runs.len(), 2);
        let cursor = first.next_cursor.expect("a next page");
        let next_query = ActivityQuery {
            limit: Some(2),
            cursor: Some(cursor),
            ..ActivityQuery::default()
        };
        let second = page(rows(3), &Filter::parse(&next_query).unwrap(), vec![]);
        assert_eq!(
            second
                .runs
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["r2"]
        );
        assert_eq!(second.next_cursor, None);
    }

    #[test]
    fn a_malformed_filter_is_refused() {
        for query in [
            ActivityQuery {
                verdict: Some("maybe".into()),
                ..ActivityQuery::default()
            },
            ActivityQuery {
                since: Some("yesterday".into()),
                ..ActivityQuery::default()
            },
            ActivityQuery {
                limit: Some(0),
                ..ActivityQuery::default()
            },
            ActivityQuery {
                limit: Some(51),
                ..ActivityQuery::default()
            },
            ActivityQuery {
                cursor: Some("nope".into()),
                ..ActivityQuery::default()
            },
        ] {
            assert!(
                matches!(Filter::parse(&query), Err(ApiError::BadRequest(_))),
                "{query:?}"
            );
        }
    }
}
