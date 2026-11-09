//! The builds the Portal starts itself (AP-130, the jc build lane): one row per build in
//! `app_builds`, in place of the forge's workflow runs.
//!
//! A build is `pending` until the dispatcher starts its Job, `running` while its pod holds the
//! build's token, `verifying` from the moment the pod says it is done (its uploads are frozen
//! then, before the Portal hashes anything), and `succeeded` or `failed` at the end. At most one
//! build of an App is pending and one running: a newer head replaces the pending one.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, PgPool};

use crate::apps::build_pods::Class;

/// One build, as the store holds it. The token itself is never held: only its SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Record {
    pub id: i64,
    pub project: String,
    pub app: String,
    pub commit: String,
    pub class: String,
    pub trigger: String,
    pub requested_by: Option<String>,
    pub state: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub digest: Option<String>,
    pub reason: Option<String>,
}

const COLUMNS: &str = "id, project, app, commit, class, trigger, requested_by, state, expires_at, \
                       created_at, started_at, finished_at, digest, reason";

impl Record {
    /// The class the build runs on.
    pub fn class(&self) -> Class {
        if self.class == "rust" {
            Class::Rust
        } else {
            Class::Node
        }
    }

    /// How long a finished build took, in seconds.
    pub fn seconds(&self) -> Option<u64> {
        let took = self.finished_at? - self.started_at?;
        u64::try_from(took.num_seconds()).ok()
    }
}

/// Why a build was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The head of the App's default branch is not the commit `status.build` names.
    Push,
    /// A person asked on the App page (AP-103).
    Rebuild,
}

impl Trigger {
    fn name(self) -> &'static str {
        match self {
            Self::Push => "push",
            Self::Rebuild => "rebuild",
        }
    }
}

/// The SHA-256 a build token is kept as.
pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Whether two hashes are equal, in time that does not depend on where they differ.
pub fn same_hash(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A fresh build token: 32 random bytes as 64 hex characters.
pub fn new_token() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Queues a build of `commit`: the App's pending build takes the new commit when there is one
/// (a newer head replaces the queued one, keeping its place), else a new one waits. Returns the
/// pending build's id.
pub async fn enqueue(
    db: &PgPool,
    project: &str,
    app: &str,
    commit: &str,
    class: Class,
    trigger: Trigger,
    requested_by: Option<&str>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO app_builds (project, app, commit, class, trigger, requested_by, state) \
         VALUES ($1, $2, $3, $4, $5, $6, 'pending') \
         ON CONFLICT (project, app) WHERE state = 'pending' DO UPDATE \
         SET commit = EXCLUDED.commit, class = EXCLUDED.class, trigger = EXCLUDED.trigger, \
             requested_by = EXCLUDED.requested_by \
         RETURNING id",
    )
    .bind(project)
    .bind(app)
    .bind(commit)
    .bind(class.name())
    .bind(trigger.name())
    .bind(requested_by)
    .fetch_one(db)
    .await
}

/// Whether any build of `commit` of the App was ever queued: a push is built once, and a
/// failed build waits for the next push or a Rebuild, as a workflow run does.
pub async fn known(db: &PgPool, project: &str, app: &str, commit: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM app_builds WHERE project = $1 AND app = $2 AND commit = $3)",
    )
    .bind(project)
    .bind(app)
    .bind(commit)
    .fetch_one(db)
    .await
}

/// One build by id.
pub async fn get(db: &PgPool, id: i64) -> Result<Option<Record>, sqlx::Error> {
    sqlx::query_as(AssertSqlSafe(format!("SELECT {COLUMNS} FROM app_builds WHERE id = $1")))
        .bind(id)
        .fetch_optional(db)
        .await
}

/// One build of the App by id: `None` for a build of another App.
pub async fn get_of(
    db: &PgPool,
    project: &str,
    app: &str,
    id: i64,
) -> Result<Option<Record>, sqlx::Error> {
    Ok(get(db, id)
        .await?
        .filter(|build| build.project == project && build.app == app))
}

/// The App's newest builds, newest first.
pub async fn latest(
    db: &PgPool,
    project: &str,
    app: &str,
    limit: i64,
) -> Result<Vec<Record>, sqlx::Error> {
    sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM app_builds WHERE project = $1 AND app = $2 ORDER BY id DESC LIMIT $3"
    )))
    .bind(project)
    .bind(app)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Every build in `state`, oldest first.
pub async fn in_state(db: &PgPool, state: &str) -> Result<Vec<Record>, sqlx::Error> {
    sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM app_builds WHERE state = $1 ORDER BY id"
    )))
    .bind(state)
    .fetch_all(db)
    .await
}

/// The oldest pending build of an App that has none running: the next to start (FIFO, one
/// build at a time per App).
pub async fn next_pending(db: &PgPool) -> Result<Option<Record>, sqlx::Error> {
    sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM app_builds p WHERE state = 'pending' AND NOT EXISTS ( \
           SELECT 1 FROM app_builds r WHERE r.project = p.project AND r.app = p.app \
           AND r.state IN ('running', 'verifying')) ORDER BY id LIMIT 1"
    )))
    .fetch_optional(db)
    .await
}

/// Starts a pending build with the hash of its token, valid for `valid_for`; `false` when it is
/// no longer pending (another pass took it).
pub async fn start(
    db: &PgPool,
    id: i64,
    token: &str,
    valid_for: chrono::Duration,
) -> Result<bool, sqlx::Error> {
    let now = Utc::now();
    let started = sqlx::query(
        "UPDATE app_builds SET state = 'running', token_sha256 = $2, started_at = $3, \
         expires_at = $4 WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(token_hash(token))
    .bind(now)
    .bind(now + valid_for)
    .execute(db)
    .await?;
    Ok(started.rows_affected() == 1)
}

/// Puts a build whose Job could not be started back in the queue, its token forgotten.
pub async fn requeue(db: &PgPool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE app_builds SET state = 'pending', token_sha256 = NULL, started_at = NULL, \
         expires_at = NULL WHERE id = $1 AND state = 'running'",
    )
    .bind(id)
    .execute(db)
    .await
    .map(|_| ())
}

/// The token hash of a build, for its routes' check.
pub async fn token_of(db: &PgPool, id: i64) -> Result<Option<Vec<u8>>, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, Option<Vec<u8>>>(
        "SELECT token_sha256 FROM app_builds WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?
    .flatten())
}

/// Freezes a running build: from here on no upload lands and its token admits nothing (its
/// hash stays, so the pod hears 409, not 404), so what the Portal hashes next is what it
/// publishes. `false` when it was not running.
pub async fn freeze(db: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    let frozen = sqlx::query(
        "UPDATE app_builds SET state = 'verifying' WHERE id = $1 AND state = 'running'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(frozen.rows_affected() == 1)
}

/// Runs `land` (an upload's last step, the rename into place) while holding the build's row
/// and only while it is running: a [`freeze`] waits for it, and one after it finds the build
/// frozen, so no upload lands once the Portal hashes. `false` when it was not running.
pub async fn while_running(
    db: &PgPool,
    id: i64,
    land: impl FnOnce() -> std::io::Result<()>,
) -> Result<bool, sqlx::Error> {
    let mut held = db.begin().await?;
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM app_builds WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *held)
            .await?;
    if state.as_deref() != Some("running") {
        return Ok(false);
    }
    land().map_err(sqlx::Error::Io)?;
    held.commit().await?;
    Ok(true)
}

/// Ends a build: `succeeded` with its digest, or `failed` with the reason; its token admits
/// nothing more. Only a build not ended already is ended.
pub async fn finish(
    db: &PgPool,
    id: i64,
    outcome: Result<&str, &str>,
) -> Result<(), sqlx::Error> {
    let (state, digest, reason) = match outcome {
        Ok(digest) => ("succeeded", Some(digest), None),
        Err(reason) => ("failed", None, Some(reason)),
    };
    sqlx::query(
        "UPDATE app_builds SET state = $2, digest = $3, reason = $4, finished_at = now() \
         WHERE id = $1 AND state IN ('pending', 'running', 'verifying')",
    )
    .bind(id)
    .bind(state)
    .bind(digest)
    .bind(reason)
    .execute(db)
    .await
    .map(|_| ())
}

/// The log the pod sent, at most [`MAX_LOG_BYTES`].
pub async fn log(db: &PgPool, id: i64) -> Result<Option<String>, sqlx::Error> {
    Ok(
        sqlx::query_scalar::<_, Option<String>>("SELECT log FROM app_builds WHERE id = $1")
            .bind(id)
            .fetch_optional(db)
            .await?
            .flatten(),
    )
}

/// The longest log a build keeps.
pub const MAX_LOG_BYTES: usize = 1024 * 1024;

/// Replaces a running build's log.
pub async fn set_log(db: &PgPool, id: i64, text: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE app_builds SET log = $2 WHERE id = $1 AND state = 'running'")
        .bind(id)
        .bind(text)
        .execute(db)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_256_random_bits_and_only_its_hash_compares() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(same_hash(&token_hash(&a), &token_hash(&a)));
        assert!(!same_hash(&token_hash(&a), &token_hash(&b)));
        assert!(!same_hash(&token_hash(&a), &token_hash(&a)[..31]));
    }
}
