//! Periodic reaper for expired agent builder runs (AG-66, AP-68).
//!
//! Runs exceeding their wall-clock lease (`expires_at`) are automatically
//! transitioned to `expired`. The reaper terminates the associated Kubernetes
//! Job, invalidates the ticket in the store, and appends a status event.

use std::time::Duration;

use crate::agents::run::AgentRunStatus;
use crate::agents::store::now_rfc3339;
use crate::state::AppState;

/// Reaps runs whose `expires_at` has passed and are still in a non-terminal state.
/// Returns the number of runs transitioned to expired.
pub async fn reap_expired(state: &AppState) -> usize {
    let now = now_rfc3339();
    let expired = match state.agents.list_expired(&now).await {
        Ok(runs) => runs,
        Err(err) => {
            tracing::error!(error = %err, "reaper: failed to list expired runs");
            return 0;
        }
    };

    let mut reaped = 0;
    for run in expired {
        match crate::api::agent_runs::end_run(state, &run, AgentRunStatus::Expired, &why(&run))
            .await
        {
            Ok(_) => {
                reaped += 1;
                tracing::info!(run_id = %run.id, "reaped expired agent run");
            }
            Err(err) => {
                tracing::warn!(run_id = %run.id, error = %err, "failed to reap expired agent run");
            }
        }
    }
    reaped
}

/// What an expired run lost, in words its owner acts on (T-2772). A run that waited for approval
/// lost only itself: its change stays open, and approving it still publishes the application.
fn why(run: &crate::agents::run::AgentRun) -> String {
    if run.status != AgentRunStatus::AwaitingApproval.as_str() {
        return "the run's time ran out before it finished; start it again from the Apps page"
            .to_owned();
    }
    let change = run
        .merge_request
        .map(|number| format!("its change #{number}"))
        .unwrap_or_else(|| "its change".to_owned());
    format!(
        "the run waited longer than its lease for {change} to be approved; the change stays open \
         under Changes, and approving it still publishes the application"
    )
}

/// Ends every run still waiting for a Change that was closed on the forge without being merged
/// (T-3015). A reject in the Portal ends its run at once; a pull request closed in the forge
/// itself is only seen here. A forge that does not answer leaves the run waiting, and its lease
/// ends it as before. Returns the number of runs ended.
pub async fn end_closed_publications(state: &AppState) -> usize {
    let waiting = match state
        .agents
        .list_in_status(AgentRunStatus::AwaitingApproval)
        .await
    {
        Ok(runs) => runs,
        Err(err) => {
            tracing::error!(error = %err, "reaper: failed to list runs waiting for approval");
            return 0;
        }
    };
    let mut ended = 0;
    for run in waiting {
        let Some(number) = run.merge_request.and_then(|n| u64::try_from(n).ok()) else {
            continue;
        };
        let Some(forge) = state.forge_for(&run.project) else {
            continue;
        };
        let pull = match forge.pull_request(number).await {
            Ok(pull) => pull,
            Err(err) => {
                tracing::debug!(run_id = %run.id, error = %err, "reaper: run's change not read");
                continue;
            }
        };
        if pull.merged || pull.state != "closed" {
            continue;
        }
        // The pull request under that number publishes this run's application, so a
        // repository that numbers its pulls anew never ends a run through another's.
        let publishes_it =
            crate::api::changes::parse_branch_name(&pull.head_branch).is_some_and(|branch| {
                branch.kind_lower == "app" && branch.resource_name == run.app_name
            });
        if !publishes_it {
            continue;
        }
        let reason = format!(
            "its change #{number} was closed on the forge without being merged; the application \
             was not published, start a new run from the Apps page to try again"
        );
        if crate::api::agent_runs::end_rejected_publication(
            state,
            &run.project,
            &run.id,
            number,
            &reason,
        )
        .await
        {
            ended += 1;
            tracing::info!(run_id = %run.id, change = number, "ended run whose change was closed");
        }
    }
    ended
}

/// Spawns the periodic background reaper loop (runs every 30 seconds, skips missed ticks).
/// Closed Changes are read from the forge every tenth tick, five minutes, one request per
/// waiting run.
pub fn spawn_periodic(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut tick: u64 = 0;
        loop {
            interval.tick().await;
            // Each replica would end the same runs and ask the forge once per waiting run
            // (F-R2POR1-011): the leader alone reaps.
            if !state.leads() {
                continue;
            }
            reap_expired(&state).await;
            if tick.is_multiple_of(10) {
                end_closed_publications(&state).await;
            }
            tick = tick.wrapping_add(1);
        }
    });
}

#[cfg(test)]
mod tests {
    //! The reaper's quiet paths (T-7818): a store that cannot list, a waiting run with no Change
    //! or no forge, a replica that does not lead, and the ticks between forge reads.

    use super::*;
    use crate::agents::store::{tests::run, AgentStore};
    use crate::config::Config;
    use std::sync::Arc;

    fn dead() -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(200))
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .expect("a lazy pool")
    }

    #[tokio::test]
    async fn a_store_that_cannot_list_reaps_nothing() {
        let mut state = AppState::new(Config::for_tests(), None);
        // An Organization in the mirror, so listing asks the database for its key and fails
        // there (an empty mirror lists nothing without asking it).
        state.mirror.upsert(crate::ops::testkit::envelope(
            "Organization",
            "hel",
            crate::permissions::ORG_NAMESPACE,
            serde_json::json!({ "installation": true }),
        ));
        state.agents = Arc::new(AgentStore::new(Some(dead()), state.mirror.clone()));
        assert!(state
            .agents
            .list_expired("2026-01-01T00:00:00Z")
            .await
            .is_err());
        assert_eq!(reap_expired(&state).await, 0);
        assert_eq!(end_closed_publications(&state).await, 0);
    }

    #[tokio::test]
    async fn a_waiting_run_without_a_change_or_a_forge_stays_waiting() {
        let state = AppState::new(Config::for_tests(), None);
        let mut waiting = run("r-1", "helsinki", "2026-09-12T10:00:00Z");
        waiting.status = AgentRunStatus::AwaitingApproval.as_str().to_owned();
        state.agents.create_run(&waiting).await.expect("a run");
        assert_eq!(end_closed_publications(&state).await, 0, "no change number");
        state
            .agents
            .set_merge_request("r-1", 5)
            .await
            .expect("its change");
        assert!(state.forge_for("helsinki").is_none());
        assert_eq!(end_closed_publications(&state).await, 0, "no forge to ask");
        let after = state
            .agents
            .get_run("r-1")
            .await
            .expect("read")
            .expect("the run");
        assert_eq!(after.status, AgentRunStatus::AwaitingApproval.as_str());
    }

    /// T-7924: a database that refuses to move a run: the expired run is not reaped and the log
    /// names it, and a run whose Change was closed on the forge is not counted as ended; both
    /// stay as they were.
    #[tokio::test]
    async fn a_store_that_refuses_the_end_reaps_and_ends_nothing() {
        use wiremock::matchers::{method, path};
        let scratch = crate::test_db::scratch().await;
        let mirror = scratch.placed("helsinki").await;
        let server = wiremock::MockServer::start().await;
        let shako =
            crate::forge::ShakoClient::new(server.uri().parse().expect("a url"), "o", "r", "t")
                .expect("a client");
        let mut state = AppState::new(Config::for_tests(), None).with_shako(Arc::new(shako));
        state.agents = Arc::new(AgentStore::new(Some(scratch.pool.clone()), mirror));
        let mut expired = run("r-old", "helsinki", "2020-01-01T00:00:00Z");
        expired.expires_at = "2020-01-01T00:20:00Z".to_owned();
        state.agents.create_run(&expired).await.expect("a run");
        let mut waiting = run("r-wait", "helsinki", "2026-09-12T10:00:00Z");
        waiting.status = AgentRunStatus::AwaitingApproval.as_str().to_owned();
        waiting.expires_at = "2999-01-01T00:00:00Z".to_owned();
        state.agents.create_run(&waiting).await.expect("a run");
        state
            .agents
            .set_merge_request("r-wait", 5)
            .await
            .expect("its change");
        wiremock::Mock::given(method("GET"))
            .and(path(
                "/odata/Repositories(Owner='o',Name='r')/PullRequests(5)",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "Number": 5, "HtmlUrl": "h", "State": "closed", "Merged": false,
                    "Title": "t", "Head": "portal/create-app-city-bikes-overview-00000005",
                    "HeadSha": "abc", "Base": "main", "BaseSha": "b"
                })),
            )
            .expect(1)
            .mount(&server)
            .await;
        scratch.refuse("agent_runs", "UPDATE").await;

        let (reaped, log) =
            crate::test_log::logged(tracing::level_filters::LevelFilter::WARN, async {
                reap_expired(&state).await
            })
            .await;
        assert_eq!(reaped, 0);
        assert!(log.contains("failed to reap expired agent run"), "{log}");
        assert!(log.contains("r-old"), "{log}");
        assert_eq!(end_closed_publications(&state).await, 0);

        for (id, status) in [
            ("r-old", AgentRunStatus::Queued),
            ("r-wait", AgentRunStatus::AwaitingApproval),
        ] {
            let after = state
                .agents
                .get_run(id)
                .await
                .expect("read")
                .expect("the run");
            assert_eq!(after.status, status.as_str(), "{id}");
        }
        drop(state);
        scratch.remove().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_leader_reaps_on_every_tick_and_a_follower_never() {
        let leader = AppState::new(Config::for_tests(), None);
        let mut expired = run("r-old", "helsinki", "2020-01-01T00:00:00Z");
        expired.expires_at = "2020-01-01T00:20:00Z".to_owned();
        leader.agents.create_run(&expired).await.expect("a run");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://follower@localhost/none")
            .expect("a lazy pool");
        let shako = Arc::new(
            crate::forge::ShakoClient::new(
                "http://localhost:3000".parse().expect("a url"),
                "o",
                "r",
                "t",
            )
            .expect("a client"),
        );
        let syncer = crate::reconciler::Syncer::new(shako, Arc::new(crate::store::Mirror::new()))
            .with_leadership(Arc::new(crate::reconciler::Leadership::reconciler(pool)));
        let follower = AppState::new(Config::for_tests(), None).with_syncer(Arc::new(syncer));
        let mut theirs = run("r-theirs", "helsinki", "2020-01-01T00:00:00Z");
        theirs.expires_at = "2020-01-01T00:20:00Z".to_owned();
        follower.agents.create_run(&theirs).await.expect("a run");
        assert!(!follower.leads());

        spawn_periodic(leader.clone());
        spawn_periodic(follower.clone());
        tokio::time::sleep(Duration::from_secs(65)).await;
        let status = |state: &AppState, id: &'static str| {
            let state = state.clone();
            async move {
                state
                    .agents
                    .get_run(id)
                    .await
                    .expect("read")
                    .expect("the run")
                    .status
            }
        };
        assert_eq!(
            status(&leader, "r-old").await,
            AgentRunStatus::Expired.as_str()
        );
        assert_eq!(
            status(&follower, "r-theirs").await,
            AgentRunStatus::Queued.as_str()
        );
    }
}
