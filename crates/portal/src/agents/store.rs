//! What a builder run and its conversation are, between the request that started it and the
//! answer that ends it (AG-43, AG-45).
//!
//! Postgres when the Portal has one, and a map when it does not, which is the shape
//! [`crate::sync::state::States`] already uses for the other loop that has to remember
//! something. A Portal without a database is the single-container development shape: a run
//! still runs and still streams, and the cost of a restart is that every in-flight run's
//! ticket is gone with it. That is the safe direction — the workspace's next proxy call is
//! refused rather than served against a run nobody can see any more — and the Job's own
//! deadline removes the pod.

use std::collections::HashMap;

use tokio::sync::RwLock;

use crate::agents::run::{AgentRun, AgentRunEvent, AgentRunStatus};
use crate::db;

/// Why a run could not be read or written. The API turns it into one 503: a builder run is not
/// something to answer half of.
///
/// This text reaches the run's page and the API's answer, so it holds no driver words; the cause
/// goes to the log where the error is made (T-6442: a person read "pool timed out ...").
#[derive(Debug, thiserror::Error)]
#[error("The Portal's database did not answer in time. Start the run again in a minute.")]
pub struct StoreError(#[source] sqlx::Error);

impl From<sqlx::Error> for StoreError {
    fn from(cause: sqlx::Error) -> Self {
        tracing::warn!(error = %cause, "the agent run store did not answer");
        Self(cause)
    }
}

/// Filter criteria for querying agent runs (AG-71).
#[derive(Debug, Clone, Default)]
pub struct RunFilter {
    pub app: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub created_by: Option<String>,
    /// `person` or `journey`; `None` lists every origin (AG-93).
    pub origin: Option<String>,
}

#[derive(Debug, Default)]
struct Memory {
    runs: HashMap<String, AgentRun>,
    /// When each run was admitted, for `first_frame_ms` without a clock parser (AP-57).
    started: HashMap<String, std::time::Instant>,
    events: HashMap<String, Vec<AgentRunEvent>>,
}

/// Every run of this Portal, by id.
#[derive(Debug)]
pub struct AgentStore {
    db: Option<sqlx::PgPool>,
    memory: RwLock<Memory>,
}

impl AgentStore {
    /// Backed by the database when there is one, by a map when there is not.
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            db,
            memory: RwLock::new(Memory::default()),
        }
    }

    /// Whether a restart keeps what this remembers, and so whether a run survives a rollout.
    pub fn is_durable(&self) -> bool {
        self.db.is_some()
    }

    pub async fn create_run(&self, run: &AgentRun) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::insert_agent_run(pool, run).await?;
            return Ok(());
        }
        let mut memory = self.memory.write().await;
        memory.runs.insert(run.id.clone(), run.clone());
        memory
            .started
            .insert(run.id.clone(), std::time::Instant::now());
        Ok(())
    }

    /// Stores a run that must be its application's only live one: `Ok(false)`, and nothing
    /// stored, when another live run of the app exists. Postgres decides under an advisory lock
    /// per App, the memory store under its write lock, so two concurrent creates never both start
    /// (F-POR1-018).
    pub async fn create_run_alone(&self, run: &AgentRun) -> Result<bool, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::insert_agent_run_alone(pool, run).await?);
        }
        let mut memory = self.memory.write().await;
        let taken = memory.runs.values().any(|r| {
            r.project == run.project
                && r.app_name == run.app_name
                && AgentRunStatus::parse(&r.status).is_some_and(|s| !s.is_terminal())
        });
        if taken {
            return Ok(false);
        }
        memory.runs.insert(run.id.clone(), run.clone());
        memory
            .started
            .insert(run.id.clone(), std::time::Instant::now());
        Ok(true)
    }

    pub async fn get_run(&self, id: &str) -> Result<Option<AgentRun>, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::load_agent_run(pool, id).await?);
        }
        Ok(self.memory.read().await.runs.get(id).cloned())
    }

    pub async fn list_runs(&self, project: &str, limit: i64) -> Result<Vec<AgentRun>, StoreError> {
        self.list_runs_filtered(project, &RunFilter::default(), limit)
            .await
    }

    /// Lists runs in a project matching the filter criteria, newest first (AG-71).
    pub async fn list_runs_filtered(
        &self,
        project: &str,
        filter: &RunFilter,
        limit: i64,
    ) -> Result<Vec<AgentRun>, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::list_agent_runs_filtered(pool, project, filter, limit).await?);
        }
        let memory = self.memory.read().await;
        let mut runs: Vec<AgentRun> = memory
            .runs
            .values()
            .filter(|run| {
                if run.project != project {
                    return false;
                }
                if let Some(app) = &filter.app {
                    if &run.app_name != app {
                        return false;
                    }
                }
                if let Some(kind) = &filter.kind {
                    if &run.kind != kind {
                        return false;
                    }
                }
                if let Some(status) = &filter.status {
                    if &run.status != status {
                        return false;
                    }
                }
                if let Some(created_by) = &filter.created_by {
                    if &run.created_by != created_by {
                        return false;
                    }
                }
                if let Some(origin) = &filter.origin {
                    if &run.origin != origin {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        // Created in the same second sorts by id, so a page is stable between two calls.
        runs.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        runs.truncate(limit.max(0) as usize);
        Ok(runs)
    }

    /// Finds the currently active (non-terminal) run for an application in a project.
    pub async fn live_run_for_app(
        &self,
        project: &str,
        app_name: &str,
    ) -> Result<Option<AgentRun>, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::load_live_agent_run_for_app(pool, project, app_name).await?);
        }
        let memory = self.memory.read().await;
        let mut matching: Vec<AgentRun> = memory
            .runs
            .values()
            .filter(|r| {
                r.project == project
                    && r.app_name == app_name
                    && AgentRunStatus::parse(&r.status).is_some_and(|s| !s.is_terminal())
            })
            .cloned()
            .collect();
        matching.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(matching.into_iter().next())
    }

    /// Lists non-terminal runs whose expiration time is before `now_rfc3339`.
    pub async fn list_expired(&self, now_rfc3339: &str) -> Result<Vec<AgentRun>, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::list_expired_agent_runs(pool, now_rfc3339).await?);
        }
        let memory = self.memory.read().await;
        let mut expired: Vec<AgentRun> = memory
            .runs
            .values()
            .filter(|r| {
                AgentRunStatus::parse(&r.status).is_some_and(|s| !s.is_terminal())
                    && r.expires_at.as_str() < now_rfc3339
            })
            .cloned()
            .collect();
        expired.sort_by(|a, b| a.expires_at.cmp(&b.expires_at));
        Ok(expired)
    }

    /// Every run in `status`, in every project, oldest first.
    pub async fn list_in_status(
        &self,
        status: AgentRunStatus,
    ) -> Result<Vec<AgentRun>, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::list_agent_runs_in_status(pool, status.as_str()).await?);
        }
        let memory = self.memory.read().await;
        let mut runs: Vec<AgentRun> = memory
            .runs
            .values()
            .filter(|run| run.status == status.as_str())
            .cloned()
            .collect();
        runs.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(runs)
    }

    /// Moves a run to `next`, refusing a transition the lifecycle does not have (AG-43).
    ///
    /// The refusal is the answer, not a log line: a cancel of a published run and a second
    /// publish of the same run both arrive here, and both are conflicts rather than writes.
    pub async fn set_status(
        &self,
        id: &str,
        next: AgentRunStatus,
        error: Option<&str>,
    ) -> Result<AgentRun, StatusChangeError> {
        if let Some(pool) = &self.db {
            let run = self
                .get_run(id)
                .await
                .map_err(StatusChangeError::Store)?
                .ok_or(StatusChangeError::Unknown)?;
            let current = transition(&run.status, next)?;
            let moved = db::update_agent_run_status(
                pool,
                id,
                current.as_str(),
                next.as_str(),
                error,
                next.is_terminal(),
            )
            .await
            .map_err(|err| StatusChangeError::Store(StoreError(err)))?;
            let after = self
                .get_run(id)
                .await
                .map_err(StatusChangeError::Store)?
                .ok_or(StatusChangeError::Unknown)?;
            if !moved {
                // Another transition won the race: refused from the status it left.
                let from =
                    AgentRunStatus::parse(&after.status).ok_or(StatusChangeError::Unknown)?;
                return Err(StatusChangeError::Refused { from, to: next });
            }
            if next.is_terminal() {
                crate::telemetry::run_finished(&run.kind, next);
            }
            return Ok(after);
        }

        // Judged and written under one lock, so two racing transitions cannot both pass.
        let mut memory = self.memory.write().await;
        let stored = memory.runs.get_mut(id).ok_or(StatusChangeError::Unknown)?;
        transition(&stored.status, next)?;
        if next.is_terminal() {
            crate::telemetry::run_finished(&stored.kind, next);
        }
        stored.status = next.as_str().to_owned();
        if let Some(message) = error {
            stored.error = Some(message.to_owned());
        }
        if stored.started_at.is_none() && next != AgentRunStatus::Queued {
            stored.started_at = Some(now_rfc3339());
        }
        if next.is_terminal() && stored.finished_at.is_none() {
            stored.finished_at = Some(now_rfc3339());
        }
        Ok(stored.clone())
    }

    /// Forgets the run's ticket hash. Every later proxy call for it verifies against a hash no
    /// ticket can produce, so a cancelled workspace cannot reach the model, the data or the
    /// forge again (AG-46).
    /// Gives a run a new lease, `expires_at` in RFC 3339 (T-2772).
    pub async fn set_expiry(&self, id: &str, expires_at: &str) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::update_agent_run_expiry(pool, id, expires_at).await?;
            return Ok(());
        }
        if let Some(run) = self.memory.write().await.runs.get_mut(id) {
            run.expires_at = expires_at.to_owned();
        }
        Ok(())
    }

    pub async fn invalidate_ticket(&self, id: &str) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::clear_agent_run_ticket(pool, id).await?;
            return Ok(());
        }
        if let Some(run) = self.memory.write().await.runs.get_mut(id) {
            run.ticket_hash.clear();
        }
        Ok(())
    }

    /// Appends one event and answers with the sequence number it got.
    pub async fn append_event(
        &self,
        run_id: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<AgentRunEvent, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::append_agent_run_event(pool, run_id, kind, &payload).await?);
        }
        let mut memory = self.memory.write().await;
        let stream = memory.events.entry(run_id.to_owned()).or_default();
        let event = AgentRunEvent {
            run_id: run_id.to_owned(),
            seq: stream.len() as i64 + 1,
            kind: kind.to_owned(),
            payload,
            created_at: now_rfc3339(),
        };
        stream.push(event.clone());
        Ok(event)
    }

    /// Everything after `after_seq`, oldest first: what a reconnecting stream missed.
    pub async fn events_since(
        &self,
        run_id: &str,
        after_seq: i64,
    ) -> Result<Vec<AgentRunEvent>, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::load_agent_run_events(pool, run_id, after_seq).await?);
        }
        Ok(self
            .memory
            .read()
            .await
            .events
            .get(run_id)
            .map(|stream| {
                stream
                    .iter()
                    .filter(|event| event.seq > after_seq)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The events of several runs of the given kinds, by run id, each run's oldest first (T-4851).
    pub async fn events_of(
        &self,
        run_ids: &[String],
        kinds: &[&str],
    ) -> Result<HashMap<String, Vec<AgentRunEvent>>, StoreError> {
        let mut by_run: HashMap<String, Vec<AgentRunEvent>> = HashMap::new();
        if run_ids.is_empty() {
            return Ok(by_run);
        }
        if let Some(pool) = &self.db {
            for event in db::load_agent_run_events_of(pool, run_ids, kinds).await? {
                by_run.entry(event.run_id.clone()).or_default().push(event);
            }
            return Ok(by_run);
        }
        let memory = self.memory.read().await;
        for id in run_ids {
            let wanted = memory
                .events
                .get(id)
                .into_iter()
                .flatten()
                .filter(|event| kinds.contains(&event.kind.as_str()))
                .cloned();
            by_run.entry(id.clone()).or_default().extend(wanted);
        }
        Ok(by_run)
    }

    /// Adds one step's token use to the run's counters (AG-44).
    pub async fn record_usage(
        &self,
        run_id: &str,
        tokens: i64,
        steps: i32,
    ) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::add_agent_run_usage(pool, run_id, tokens, steps).await?;
            return Ok(());
        }
        if let Some(run) = self.memory.write().await.runs.get_mut(run_id) {
            run.tokens_used += tokens;
            run.steps += steps;
        }
        Ok(())
    }

    pub async fn set_preview_url(&self, run_id: &str, url: &str) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::set_agent_run_preview_url(pool, run_id, url).await?;
            return Ok(());
        }
        let mut memory = self.memory.write().await;
        let elapsed = memory
            .started
            .get(run_id)
            .map(|since| i64::try_from(since.elapsed().as_millis()).unwrap_or(i64::MAX));
        if let Some(run) = memory.runs.get_mut(run_id) {
            run.preview_url = Some(url.to_owned());
            if run.first_frame_ms.is_none() {
                run.first_frame_ms = elapsed;
            }
        }
        Ok(())
    }

    /// Records when the first generated version of an application was served.
    pub async fn record_first_version(&self, run_id: &str) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::record_agent_run_first_version(pool, run_id).await?;
            return Ok(());
        }
        let mut memory = self.memory.write().await;
        let elapsed = memory
            .started
            .get(run_id)
            .map(|since| i64::try_from(since.elapsed().as_millis()).unwrap_or(i64::MAX));
        if let Some(run) = memory.runs.get_mut(run_id) {
            if run.first_version_ms.is_none() {
                run.first_version_ms = elapsed;
            }
        }
        Ok(())
    }

    /// The files a run wrote; the preview is rendered from them (AP-56).
    pub async fn set_files(
        &self,
        run_id: &str,
        files: serde_json::Value,
    ) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::set_agent_run_files(pool, run_id, &files).await?;
            return Ok(());
        }
        if let Some(run) = self.memory.write().await.runs.get_mut(run_id) {
            run.files = files;
        }
        Ok(())
    }

    /// The application's name for people, chosen with its first version.
    pub async fn set_title(&self, run_id: &str, title: &str) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::set_agent_run_title(pool, run_id, title).await?;
            return Ok(());
        }
        if let Some(run) = self.memory.write().await.runs.get_mut(run_id) {
            run.title = Some(title.to_owned());
        }
        Ok(())
    }

    /// The endpoints a conversation reads from the next message on (AG-75).
    pub async fn set_endpoints(
        &self,
        run_id: &str,
        endpoints: &[crate::agents::endpoints::RunEndpoint],
    ) -> Result<(), StoreError> {
        let value = serde_json::to_value(endpoints).unwrap_or_else(|_| serde_json::json!([]));
        let (name, slug) = endpoints
            .first()
            .map(|e| (e.name.clone(), e.slug.clone()))
            .unwrap_or_default();
        if let Some(pool) = &self.db {
            db::set_agent_run_endpoints(pool, run_id, &value, &name, &slug).await?;
            return Ok(());
        }
        if let Some(run) = self.memory.write().await.runs.get_mut(run_id) {
            run.endpoints = value;
            run.endpoint_name = name;
            run.endpoint_slug = slug;
        }
        Ok(())
    }

    pub async fn set_merge_request(&self, run_id: &str, number: i32) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::set_agent_run_merge_request(pool, run_id, number).await?;
            return Ok(());
        }
        if let Some(run) = self.memory.write().await.runs.get_mut(run_id) {
            run.merge_request = Some(number);
        }
        Ok(())
    }
}

/// Why a state change did not happen.
#[derive(Debug, thiserror::Error)]
pub enum StatusChangeError {
    #[error("no such run, or a state this Portal does not know")]
    Unknown,
    #[error("a run in '{from}' does not move to '{to}'", from = from.as_str(), to = to.as_str())]
    Refused {
        from: AgentRunStatus,
        to: AgentRunStatus,
    },
    #[error(transparent)]
    Store(StoreError),
}

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// The run's status as read, when it may move to `next` (AG-43); the refusal otherwise.
fn transition(status: &str, next: AgentRunStatus) -> Result<AgentRunStatus, StatusChangeError> {
    let current = AgentRunStatus::parse(status).ok_or(StatusChangeError::Unknown)?;
    if current.allows_transition_to(next) {
        Ok(current)
    } else {
        Err(StatusChangeError::Refused {
            from: current,
            to: next,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-6442: this text reaches the run's page; a person read "pool timed out while waiting for
    /// an open connection" beside a stopped run.
    #[test]
    fn a_store_failure_reads_in_a_persons_words() {
        let shown = StoreError::from(sqlx::Error::PoolTimedOut).to_string();
        assert!(!shown.contains("pool"), "{shown}");
        assert!(shown.contains("Start the run again"), "{shown}");
    }

    fn run(id: &str, project: &str, created_at: &str) -> AgentRun {
        AgentRun {
            id: id.to_owned(),
            project: project.to_owned(),
            app_name: "city-bikes-overview".to_owned(),
            title: None,
            endpoint_name: "helsinki-bikes".to_owned(),
            endpoint_slug: "si6epqkx364lprho5uaigutk274r5grb".to_owned(),
            endpoints: serde_json::json!([]),
            profile: "app-builder".to_owned(),
            kind: "application".to_owned(),
            unattended: false,
            continues: None,
            app_class: "ui".to_owned(),
            visibility: "project".to_owned(),
            prompt: "a live bike availability dashboard".to_owned(),
            prompt_digest: "sha256:abc".to_owned(),
            data_needs: serde_json::json!([]),
            allows_write: false,
            branch: format!("agent/app-city-bikes-overview/{id}"),
            path_prefix: "projects/helsinki/apps/city-bikes-overview/".to_owned(),
            status: AgentRunStatus::Queued.as_str().to_owned(),
            ticket_hash: "$argon2id$stub".to_owned(),
            workspace: None,
            merge_request: None,
            change_id: None,
            source_url: None,
            mirror_url: None,
            preview_url: None,
            first_frame_ms: None,
            first_version_ms: None,
            files: serde_json::json!({}),
            steps: 0,
            tokens_used: 0,
            created_by: "demo.steward@hel.fi".to_owned(),
            origin: "person".to_owned(),
            starter: serde_json::Value::Null,
            created_at: created_at.to_owned(),
            started_at: None,
            finished_at: None,
            expires_at: "2026-09-12T10:35:30Z".to_owned(),
            error: None,
        }
    }

    /// F-POR1-019: two terminal transitions of one run race; exactly one wins and the other is
    /// refused from the status the winner left.
    #[tokio::test]
    async fn of_two_racing_endings_exactly_one_wins() {
        let store = std::sync::Arc::new(AgentStore::new(None));
        store
            .create_run(&run("run-1", "helsinki", "2026-09-12T10:15:30Z"))
            .await
            .expect("create");
        let (a, b) = (store.clone(), store.clone());
        let (cancel, fail) = tokio::join!(
            tokio::spawn(
                async move { a.set_status("run-1", AgentRunStatus::Cancelled, None).await }
            ),
            tokio::spawn(async move {
                b.set_status("run-1", AgentRunStatus::Failed, Some("x"))
                    .await
            }),
        );
        let outcomes = [cancel.expect("joined"), fail.expect("joined")];
        assert_eq!(
            outcomes.iter().filter(|o| o.is_ok()).count(),
            1,
            "{outcomes:?}"
        );
        assert!(outcomes
            .iter()
            .any(|o| matches!(o, Err(StatusChangeError::Refused { .. }))));
    }

    /// F-POR1-018: an App holds one live run; a second create waits until the first has ended,
    /// and another App of the project is never held by it.
    #[tokio::test]
    async fn a_live_run_holds_its_app_against_a_second_run() {
        let store = AgentStore::new(None);
        let first = run("run-1", "helsinki", "2026-09-12T10:15:30Z");
        assert!(store.create_run_alone(&first).await.expect("create"));
        assert!(!store
            .create_run_alone(&run("run-2", "helsinki", "2026-09-12T10:16:30Z"))
            .await
            .expect("create"));
        assert!(store.get_run("run-2").await.expect("get").is_none());

        let mut other_app = run("run-3", "helsinki", "2026-09-12T10:17:30Z");
        other_app.app_name = "air-quality".to_owned();
        assert!(store.create_run_alone(&other_app).await.expect("create"));

        let mut ended = run("run-4", "bb", "2026-09-12T10:18:30Z");
        ended.status = AgentRunStatus::Failed.as_str().to_owned();
        store.create_run(&ended).await.expect("create");
        assert!(store
            .create_run_alone(&run("run-5", "bb", "2026-09-12T10:19:30Z"))
            .await
            .expect("create"));
    }

    #[tokio::test]
    async fn a_run_is_read_back_and_listed_inside_its_own_project() {
        let store = AgentStore::new(None);
        assert!(!store.is_durable());
        store
            .create_run(&run("run-1", "helsinki", "2026-09-12T10:15:30Z"))
            .await
            .expect("create");
        store
            .create_run(&run("run-2", "helsinki", "2026-09-12T10:16:30Z"))
            .await
            .expect("create");
        store
            .create_run(&run("run-3", "bb", "2026-09-12T10:17:30Z"))
            .await
            .expect("create");

        let listed = store.list_runs("helsinki", 10).await.expect("list");
        assert_eq!(
            listed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["run-2", "run-1"],
            "newest first, and another project's run is not in this project's list"
        );
        assert_eq!(store.list_runs("helsinki", 1).await.expect("list").len(), 1);
        assert!(store.get_run("run-4").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn the_lifecycle_is_what_the_store_allows() {
        let store = AgentStore::new(None);
        store
            .create_run(&run("run-1", "helsinki", "2026-09-12T10:15:30Z"))
            .await
            .expect("create");

        let started = store
            .set_status("run-1", AgentRunStatus::Starting, None)
            .await
            .expect("queued moves to starting");
        assert_eq!(started.status, "starting");
        assert!(
            started.started_at.is_some(),
            "the first state after queued stamps started_at"
        );
        assert!(started.finished_at.is_none());

        let err = store
            .set_status("run-1", AgentRunStatus::Published, None)
            .await
            .expect_err("starting does not jump to published");
        assert!(matches!(err, StatusChangeError::Refused { .. }));

        let cancelled = store
            .set_status("run-1", AgentRunStatus::Cancelled, Some("user cancelled"))
            .await
            .expect("any live state may be cancelled");
        assert_eq!(cancelled.error.as_deref(), Some("user cancelled"));
        assert!(
            cancelled.finished_at.is_some(),
            "a terminal state stamps finished_at"
        );

        for next in [AgentRunStatus::Building, AgentRunStatus::Cancelled] {
            let err = store
                .set_status("run-1", next, None)
                .await
                .expect_err("a cancelled run is over, a second cancel included");
            assert!(matches!(err, StatusChangeError::Refused { .. }));
        }
    }

    #[tokio::test]
    async fn cancelling_leaves_a_ticket_hash_nothing_can_match() {
        let store = AgentStore::new(None);
        store
            .create_run(&run("run-1", "helsinki", "2026-09-12T10:15:30Z"))
            .await
            .expect("create");

        store.invalidate_ticket("run-1").await.expect("invalidate");
        let after = store.get_run("run-1").await.expect("get").expect("run");
        assert!(
            after.ticket_hash.is_empty(),
            "an empty hash is not a PHC string, so no ticket ever verifies against it"
        );
    }

    #[tokio::test]
    async fn the_stream_numbers_events_and_replays_only_what_was_missed() {
        let store = AgentStore::new(None);
        let first = store
            .append_event("run-1", "status", serde_json::json!({"status": "starting"}))
            .await
            .expect("append");
        let second = store
            .append_event("run-1", "thought", serde_json::json!({"text": "reading"}))
            .await
            .expect("append");
        assert_eq!((first.seq, second.seq), (1, 2));

        let missed = store.events_since("run-1", 1).await.expect("since");
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].seq, 2);
        assert!(
            store
                .events_since("run-1", 2)
                .await
                .expect("since")
                .is_empty(),
            "a stream that is up to date replays nothing"
        );
        assert!(
            store
                .events_since("other-run", 0)
                .await
                .expect("since")
                .is_empty(),
            "one run's stream is not another's"
        );
    }

    #[tokio::test]
    async fn usage_accumulates_across_steps() {
        let store = AgentStore::new(None);
        store
            .create_run(&run("run-1", "helsinki", "2026-09-12T10:15:30Z"))
            .await
            .expect("create");
        store.record_usage("run-1", 4_120, 1).await.expect("usage");
        store.record_usage("run-1", 1_880, 1).await.expect("usage");
        let after = store.get_run("run-1").await.expect("get").expect("run");
        assert_eq!((after.tokens_used, after.steps), (6_000, 2));
    }

    #[tokio::test]
    async fn live_and_expired_runs_are_correctly_queried() {
        let store = AgentStore::new(None);
        let mut r1 = run("run-1", "helsinki", "2026-09-12T10:15:30Z");
        r1.expires_at = "2026-09-12T10:20:00Z".to_owned();
        let mut r2 = run("run-2", "helsinki", "2026-09-12T10:16:30Z");
        r2.expires_at = "2026-09-12T10:50:00Z".to_owned();
        store.create_run(&r1).await.expect("create r1");
        store.create_run(&r2).await.expect("create r2");

        // live_run_for_app picks newest non-terminal run
        let live = store
            .live_run_for_app("helsinki", "city-bikes-overview")
            .await
            .expect("live_run")
            .expect("found");
        assert_eq!(live.id, "run-2");

        // list_expired with cutoff
        let expired = store
            .list_expired("2026-09-12T10:30:00Z")
            .await
            .expect("list_expired");
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, "run-1");

        // record_first_version sets first_version_ms once
        store
            .record_first_version("run-1")
            .await
            .expect("record_first_version");
        let after_v1 = store.get_run("run-1").await.expect("get").expect("exists");
        assert!(after_v1.first_version_ms.is_some());
    }

    #[tokio::test]
    async fn filtered_runs_filter_by_kind_and_creator() {
        let store = AgentStore::new(None);
        let mut r1 = run("run-1", "helsinki", "2026-09-12T10:15:30Z");
        r1.kind = "conversation".to_owned();
        r1.created_by = "alice".to_owned();
        let mut r2 = run("run-2", "helsinki", "2026-09-12T10:16:30Z");
        r2.kind = "application".to_owned();
        r2.created_by = "bob".to_owned();
        store.create_run(&r1).await.expect("create r1");
        store.create_run(&r2).await.expect("create r2");

        let convos = store
            .list_runs_filtered(
                "helsinki",
                &RunFilter {
                    kind: Some("conversation".into()),
                    ..Default::default()
                },
                10,
            )
            .await
            .expect("filter kind");
        assert_eq!(convos.len(), 1);
        assert_eq!(convos[0].id, "run-1");

        let alice_runs = store
            .list_runs_filtered(
                "helsinki",
                &RunFilter {
                    created_by: Some("alice".into()),
                    ..Default::default()
                },
                10,
            )
            .await
            .expect("filter creator");
        assert_eq!(alice_runs.len(), 1);
        assert_eq!(alice_runs[0].id, "run-1");
    }

    #[tokio::test]
    async fn events_of_keeps_the_asked_kinds_of_each_run_oldest_first() {
        let store = AgentStore::new(None);
        for (run, kind) in [
            ("run-1", "status"),
            ("run-1", "thought"),
            ("run-2", "status"),
            ("run-1", "status"),
            ("run-3", "status"),
        ] {
            store
                .append_event(run, kind, serde_json::json!({}))
                .await
                .expect("append");
        }
        let asked = ["run-1".to_owned(), "run-2".to_owned(), "run-9".to_owned()];
        let by_run = store.events_of(&asked, &["status"]).await.expect("events");
        let seqs = |run: &str| by_run[run].iter().map(|e| e.seq).collect::<Vec<_>>();
        assert_eq!(seqs("run-1"), [1, 3], "the thought is left out");
        assert_eq!(seqs("run-2"), [1]);
        assert!(
            by_run["run-9"].is_empty(),
            "a run with no events is an empty list"
        );
        assert!(
            !by_run.contains_key("run-3"),
            "a run not asked for is not read"
        );
        assert!(store
            .events_of(&[], &["status"])
            .await
            .expect("events")
            .is_empty());
    }
}
