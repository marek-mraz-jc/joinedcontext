//! The pipeline phases the reconciling replica last set, for the replicas that do not reconcile
//! (T-2976, OPS-51).
//!
//! Only the leader deploys streams, so only it knows whether a pipeline is Live, stalled or
//! refused. A follower used to call every stream pipeline Pending, which during a rollout showed
//! the whole organization waiting for approval while its pipelines ran. The leader now writes what
//! it concluded after every run, and a follower serves that, as long as it is fresh.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::resource::{Condition, Phase};

/// How old the leader's word may be before a follower stops trusting it: several sync intervals,
/// so a slow run does not flip the answer, and short enough that a leader gone for good is not
/// reported Live for long.
pub const FRESH: Duration = Duration::from_secs(600);

/// What the leader concluded about one pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub phase: Phase,
    pub conditions: Vec<Condition>,
}

/// `(project, pipeline)`.
pub type Key = (String, String);

/// The shared record, in the Portal's database.
#[derive(Debug, Clone)]
pub struct Store {
    db: sqlx::PgPool,
}

impl Store {
    pub fn new(db: sqlx::PgPool) -> Self {
        Self { db }
    }

    /// Replaces the record with `statuses`, the leader's whole answer of this run: a pipeline it
    /// no longer holds leaves the record with it.
    pub async fn save(&self, statuses: &HashMap<Key, Saved>) -> Result<(), sqlx::Error> {
        let mut tx = self.db.begin().await?;
        sqlx::query("DELETE FROM pipeline_status")
            .execute(&mut *tx)
            .await?;
        for ((project, pipeline), saved) in statuses {
            let status =
                serde_json::to_value(saved).map_err(|err| sqlx::Error::Encode(Box::new(err)))?;
            sqlx::query(
                "INSERT INTO pipeline_status (project, pipeline, status) VALUES ($1, $2, $3)",
            )
            .bind(project)
            .bind(pipeline)
            .bind(status)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// What the leader said within `fresh`. A row that no longer parses is skipped, never fatal.
    pub async fn load(&self, fresh: Duration) -> Result<HashMap<Key, Saved>, sqlx::Error> {
        let seconds = i64::try_from(fresh.as_secs()).unwrap_or(i64::MAX);
        let rows: Vec<(String, String, serde_json::Value)> = sqlx::query_as(
            "SELECT project, pipeline, status FROM pipeline_status \
             WHERE updated_at > now() - make_interval(secs => $1::double precision)",
        )
        .bind(seconds as f64)
        .fetch_all(&self.db)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(project, pipeline, status)| {
                Some(((project, pipeline), serde_json::from_value(status).ok()?))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database_url() -> Option<String> {
        std::env::var("JC_PORTAL_TEST_DATABASE_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
    }

    fn condition() -> Condition {
        serde_json::from_value(serde_json::json!({
            "type": "StreamWriting",
            "status": "False",
            "reason": "Stalled",
            "message": "records in, nothing out",
            "lastTransitionTime": "2026-09-25T18:00:00Z"
        }))
        .expect("a condition")
    }

    /// The leader's answer comes back whole, a later answer replaces it, and nothing is fresh
    /// once the window is zero.
    #[tokio::test]
    async fn a_follower_reads_what_the_leader_saved_while_it_is_fresh() {
        let Some(url) = database_url() else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let store = Store::new(crate::db::connect(&url).await.expect("connect and migrate"));
        let live = Saved {
            phase: Phase::Live,
            conditions: vec![condition()],
        };
        let key = ("t2976".to_owned(), "hsl-hfp-vehicles".to_owned());
        store
            .save(&HashMap::from([(key.clone(), live.clone())]))
            .await
            .expect("save");
        assert_eq!(
            store.load(FRESH).await.expect("load").get(&key),
            Some(&live)
        );

        let gone = ("t2976".to_owned(), "retired".to_owned());
        store
            .save(&HashMap::from([(
                gone.clone(),
                Saved {
                    phase: Phase::Error,
                    conditions: Vec::new(),
                },
            )]))
            .await
            .expect("save again");
        let loaded = store.load(FRESH).await.expect("load");
        assert!(
            !loaded.contains_key(&key),
            "a pipeline the leader no longer holds is dropped"
        );
        assert_eq!(loaded.get(&gone).map(|s| s.phase), Some(Phase::Error));

        assert!(
            store.load(Duration::ZERO).await.expect("load").is_empty(),
            "a stale answer is not served"
        );
    }
}
