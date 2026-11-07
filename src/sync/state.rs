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
#[derive(Debug)]
pub struct States {
    db: Option<sqlx::PgPool>,
    memory: Mutex<HashMap<(String, String), Stored>>,
}

impl States {
    /// Backed by the database when there is one, by a map when there is not.
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
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
        if let Some(pool) = &self.db {
            return match db::load_sync_state(pool, namespace, name).await {
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
            .ok()
            .and_then(|map| map.get(&key(namespace, name)).cloned())
            .unwrap_or_default()
    }

    /// Records one source's memory. A write that fails is loud: the next run repeats work a
    /// reviewer has already seen, and the log is where that is explained.
    pub async fn put(&self, namespace: &str, name: &str, stored: &Stored) {
        if let Some(pool) = &self.db {
            if let Err(err) = db::save_sync_state(pool, &stored.to_row(namespace, name)).await {
                tracing::warn!(%namespace, %name, error = %err, "sync state was not recorded");
            }
            return;
        }
        if let Ok(mut map) = self.memory.lock() {
            map.insert(key(namespace, name), stored.clone());
        }
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
