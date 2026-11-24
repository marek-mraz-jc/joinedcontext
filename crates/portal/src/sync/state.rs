//! What the `SyncSource` loop remembers between ticks (MF-30).
//!
//! `jcctl::sync` holds no state on purpose: a run is a decision, and the caller is what
//! survives between two of them. This is that caller's memory, and it has to outlive the
//! process — a Portal that forgot which revision it had already proposed would open the same
//! merge request again after every restart, which is exactly the queue CC-18 forbids.
//!
//! Postgres when the Portal has one, and a map when it does not. A Portal without a database
//! is the single-container development shape: the loop still works, the operator's **Pause**
//! still holds until the process ends, and the cost of a restart is one duplicate proposal per
//! source with a run in flight. That is written down here rather than hidden, because the
//! deployment decides which of the two it gets.

use std::collections::HashMap;
use std::sync::Mutex;

use jcctl::sync::{Phase, State};

use crate::db::{self, SyncStateRow};
use crate::store::Mirror;

/// One source's memory: the state `jcctl::sync` reads, and the two things it has no field for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stored {
    /// What [`jcctl::sync::poll`] is handed.
    pub state: State,
    /// The full revision the open proposal carries; its branch name holds only the first
    /// seven characters, and the merge is what makes this the observed revision.
    pub open_revision: Option<String>,
    /// Where a reviewer answers the open proposal (MF-30).
    pub merge_request: Option<String>,
    /// Why the last run did not finish. What makes the `Error` phase say something rather than
    /// leaving an operator to read the Portal's log (MF-30).
    pub last_error: Option<String>,
}

impl Stored {
    /// What the project page shows for this source (MF-30).
    ///
    /// Derived rather than stored: a phase written down beside the facts it summarises is a
    /// second copy of the truth, and the two disagree the first time a write is interrupted.
    pub fn phase(&self) -> Phase {
        if self.state.paused {
            return Phase::Paused;
        }
        if self.state.open_proposal.is_some() {
            return Phase::PendingApproval;
        }
        if self.last_error.is_some() {
            return Phase::Error;
        }
        if self.state.observed_revision.is_some() {
            return Phase::Synced;
        }
        // Never run, or run and left carrying nothing: either way the repository does not yet
        // hold what the source publishes.
        Phase::OutOfSync
    }

    fn from_row(row: SyncStateRow) -> Self {
        Self {
            state: State {
                observed_revision: row.observed_revision,
                last_run_at: row.last_run_at.map(|at| at.max(0) as u64),
                open_proposal: row.open_proposal,
                paused: row.paused,
            },
            open_revision: row.open_revision,
            merge_request: row.merge_request,
            last_error: row.last_error,
        }
    }

    fn to_row(&self, namespace: &str, name: &str) -> SyncStateRow {
        SyncStateRow {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
            observed_revision: self.state.observed_revision.clone(),
            last_run_at: self.state.last_run_at.map(|at| at as i64),
            open_proposal: self.state.open_proposal.clone(),
            open_revision: self.open_revision.clone(),
            merge_request: self.merge_request.clone(),
            last_error: self.last_error.clone(),
            paused: self.state.paused,
        }
    }
}

/// The memory of every `SyncSource`, by project and name.
pub struct States {
    /// The database and the mirror that places each project's key: a row is read and written
    /// only inside its project's `db::tenant_tx` (T-7281).
    db: Option<(sqlx::PgPool, std::sync::Arc<Mirror>)>,
    memory: Mutex<HashMap<(String, String), Stored>>,
}

impl std::fmt::Debug for States {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("States")
            .field("durable", &self.db.is_some())
            .finish_non_exhaustive()
    }
}

impl States {
    /// Backed by the database when there is one, by a map when there is not.
    pub fn new(db: Option<(sqlx::PgPool, std::sync::Arc<Mirror>)>) -> Self {
        Self {
            db,
            memory: Mutex::new(HashMap::new()),
        }
    }

    /// Whether a restart keeps what this remembers.
    pub fn is_durable(&self) -> bool {
        self.db.is_some()
    }

    /// One source's memory, default before its first run.
    ///
    /// A database that cannot be read is logged and answered as "nothing recorded": the run
    /// that follows re-proposes at worst, where returning an error would stop every source of
    /// the tick because one row could not be read.
    pub async fn get(&self, namespace: &str, name: &str) -> Stored {
        if let Some((pool, mirror)) = &self.db {
            return match Self::load(pool, mirror, namespace, name).await {
                Ok(Some(row)) => Stored::from_row(row),
                Ok(None) => Stored::default(),
                Err(err) => {
                    tracing::warn!(%namespace, %name, error = %err, "sync state did not load");
                    Stored::default()
                }
            };
        }
        self.memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key(namespace, name))
            .cloned()
            .unwrap_or_default()
    }

    /// Records one source's memory. A write that fails is loud: the next run repeats work a
    /// reviewer has already seen, and the log is where that is explained.
    pub async fn put(&self, namespace: &str, name: &str, stored: &Stored) {
        if let Some((pool, mirror)) = &self.db {
            match Self::save(pool, mirror, &stored.to_row(namespace, name)).await {
                Ok(true) => {}
                Ok(false) => tracing::warn!(
                    %namespace, %name,
                    "sync state was not recorded: the project has no key (no Project in the \
                     mirror or its Organization is not an active one of the registry)"
                ),
                Err(err) => {
                    tracing::warn!(%namespace, %name, error = %err, "sync state was not recorded");
                }
            }
            return;
        }
        self.memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key(namespace, name), stored.clone());
    }

    /// Switches syncing off or on for one source (MF-30, the **Pause** button).
    ///
    /// A pause is an operator's decision about a running loop, not a change to the manifest,
    /// so it lives here rather than in a merge request. Detaching a source for good is the
    /// other button, and that one is a change to the repository.
    pub async fn set_paused(&self, namespace: &str, name: &str, paused: bool) {
        let mut stored = self.get(namespace, name).await;
        stored.state.paused = paused;
        self.put(namespace, name, &stored).await;
    }
}

impl States {
    /// The row of `namespace`/`name` read in the project's key; `None` for a project with none.
    async fn load(
        pool: &sqlx::PgPool,
        mirror: &Mirror,
        namespace: &str,
        name: &str,
    ) -> Result<Option<SyncStateRow>, sqlx::Error> {
        let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, namespace).await? else {
            return Ok(None);
        };
        let row = db::load_sync_state(&mut tx, namespace, name).await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Writes `row` in its project's key; `false` when the project has none.
    async fn save(
        pool: &sqlx::PgPool,
        mirror: &Mirror,
        row: &SyncStateRow,
    ) -> Result<bool, sqlx::Error> {
        let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, &row.namespace).await? else {
            return Ok(false);
        };
        db::save_sync_state(&mut tx, row).await?;
        tx.commit().await?;
        Ok(true)
    }
}

fn key(namespace: &str, name: &str) -> (String, String) {
    (namespace.to_owned(), name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_source_with_no_run_behind_it_starts_from_nothing() {
        let states = States::new(None);
        assert_eq!(states.get("bb", "regional").await, Stored::default());
        assert!(!states.is_durable());
        assert_eq!(
            format!("{states:?}"),
            "States { durable: false, .. }",
            "the debug form names no row"
        );
    }

    #[tokio::test]
    async fn what_a_run_recorded_is_what_the_next_run_reads() {
        let states = States::new(None);
        let stored = Stored {
            state: State {
                observed_revision: Some("c0ffee".into()),
                last_run_at: Some(1_000),
                open_proposal: Some("chg-sync-regional-c0ffee".into()),
                paused: false,
            },
            open_revision: Some("c0ffee1234".into()),
            merge_request: Some("https://git.example/org/config/pulls/7".into()),
            last_error: None,
        };
        states.put("bb", "regional", &stored).await;

        assert_eq!(states.get("bb", "regional").await, stored);
        assert_eq!(
            states.get("helsinki", "regional").await,
            Stored::default(),
            "two projects may run a source of the same name"
        );
    }

    fn recorded() -> Stored {
        Stored {
            state: State {
                observed_revision: Some("c0ffee".into()),
                last_run_at: Some(1_000),
                open_proposal: Some("chg-sync-regional-c0ffee".into()),
                paused: true,
            },
            open_revision: Some("c0ffee1234".into()),
            merge_request: Some("https://git.example/org/config/pulls/7".into()),
            last_error: Some("the forge answered 502".into()),
        }
    }

    /// T-7281: in Postgres a run's memory is kept in its project's key and read back whole; a
    /// project with no key keeps nothing and reads as never run.
    #[tokio::test]
    async fn the_database_keeps_a_runs_memory_in_its_projects_key() {
        let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
        else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect and migrate");
        let run = jc_types::ProjectId::mint().path_segment();
        let project = format!("t7656-{run}");
        let mirror = std::sync::Arc::new(Mirror::new());
        crate::project_ids::placed(&pool, &mirror, &project, &format!("t7656org-{run}")).await;
        let states = States::new(Some((pool, mirror)));
        assert!(states.is_durable());
        assert_eq!(states.get(&project, "regional").await, Stored::default());
        states.put(&project, "regional", &recorded()).await;
        assert_eq!(states.get(&project, "regional").await, recorded());
        states.set_paused(&project, "regional", false).await;
        assert!(!states.get(&project, "regional").await.state.paused);
        let unplaced = format!("t7656-unplaced-{run}");
        states.put(&unplaced, "regional", &recorded()).await;
        assert_eq!(states.get(&unplaced, "regional").await, Stored::default());
    }

    /// A database that does not answer reads as nothing recorded and a write is only logged:
    /// one row never stops the tick.
    #[tokio::test]
    async fn a_database_that_does_not_answer_reads_as_never_run() {
        let closed = tokio::net::TcpSocket::new_v4().unwrap();
        closed.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(2))
            .connect_lazy(&format!(
                "postgres://portal@{}/none",
                closed.local_addr().unwrap()
            ))
            .expect("a lazy pool");
        let mirror = std::sync::Arc::new(Mirror::new());
        mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "Project".to_owned(),
            metadata: crate::resource::ObjectMeta::new("bb", crate::permissions::ORG_NAMESPACE),
            spec: serde_json::json!({ "organizationRef": "bbsk" }),
            status: None,
        });
        let states = States::new(Some((pool, mirror)));
        states.put("bb", "regional", &recorded()).await;
        assert_eq!(states.get("bb", "regional").await, Stored::default());
    }

    /// A thread that panicked holding the memory leaves it usable: what a run recorded is still
    /// what the next run reads.
    #[tokio::test]
    async fn a_poisoned_memory_still_records_and_reads() {
        let states = States::new(None);
        std::thread::scope(|s| {
            let _ = s
                .spawn(|| {
                    let _held = states.memory.lock().unwrap();
                    panic!("poison the memory");
                })
                .join();
        });
        states.put("bb", "regional", &recorded()).await;
        assert_eq!(states.get("bb", "regional").await, recorded());
    }

    #[test]
    fn the_phase_is_what_the_memory_says_and_pause_wins() {
        assert_eq!(Stored::default().phase(), Phase::OutOfSync);

        let mut stored = Stored {
            state: State {
                observed_revision: Some("c0ffee".into()),
                last_run_at: Some(1_000),
                ..State::default()
            },
            ..Stored::default()
        };
        assert_eq!(stored.phase(), Phase::Synced);

        stored.last_error = Some("the origin did not answer in time".into());
        assert_eq!(stored.phase(), Phase::Error);

        stored.state.open_proposal = Some("chg-sync-regional-c0ffee".into());
        assert_eq!(
            stored.phase(),
            Phase::PendingApproval,
            "an open proposal is what the source is waiting on, whatever happened before it"
        );

        stored.state.paused = true;
        assert_eq!(
            stored.phase(),
            Phase::Paused,
            "a paused source is paused however much is outstanding"
        );
    }

    #[tokio::test]
    async fn pausing_leaves_the_rest_of_the_memory_alone() {
        let states = States::new(None);
        let stored = Stored {
            state: State {
                observed_revision: Some("c0ffee".into()),
                last_run_at: Some(1_000),
                ..State::default()
            },
            ..Stored::default()
        };
        states.put("bb", "regional", &stored).await;

        states.set_paused("bb", "regional", true).await;
        let after = states.get("bb", "regional").await;
        assert!(after.state.paused);
        assert_eq!(
            after.state.observed_revision.as_deref(),
            Some("c0ffee"),
            "a pause must not make the next run re-import the revision already carried"
        );

        states.set_paused("bb", "regional", false).await;
        assert!(!states.get("bb", "regional").await.state.paused);
    }
}
