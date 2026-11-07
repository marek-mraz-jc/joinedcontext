//! Which replica may reconcile (T-0191, CC-03, CC-55).
//!
//! Several Portal replicas serve the UI, but only one may write to the platform: two
//! reconcilers converging the same repository would fight over every resource. The election
//! is a PostgreSQL advisory lock, which needs no new component and no lease arithmetic. The
//! lock lives on a database session, so a leader that crashes, is evicted or loses its
//! network releases it the moment its connection dies, and the next replica wins the lock on
//! its following tick.
//!
//! The leader holds one connection of the pool for as long as it leads. That is the price of
//! a session-scoped lock; the pool is sized for it.
//!
//! A dead client does not always close its session: a pod killed behind its Linkerd proxy
//! sends no FIN, and PostgreSQL would keep the session, and the lock, until TCP keepalive
//! gives up about two hours later. So the lock session carries a lease the server enforces by
//! itself, `idle_session_timeout`, and the leader keeps it busy with a heartbeat. A leader
//! that stops beating loses the lock one [`LEASE`] later, whatever the network did.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::pool::PoolConnection;
use sqlx::{Connection, PgPool, Postgres};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// The advisory lock the reconciler competes for.
///
/// PostgreSQL advisory locks share one 64-bit namespace with everything else in the
/// database, so the key is a literal rather than a hash: `jc_recon` in ASCII, which is
/// readable in `pg_locks` when an operator asks who holds it.
pub const RECONCILER_LOCK_KEY: i64 = 0x6a63_5f72_6563_6f6e;

/// How long the server keeps the lock of a leader that went silent. The heartbeat beats three
/// times per lease, so one slow beat does not cost a live leader its lock.
pub const LEASE: Duration = Duration::from_secs(60);

type Held = Arc<Mutex<Option<PoolConnection<Postgres>>>>;

/// One replica's claim on the reconciler role.
pub struct Leadership {
    pool: PgPool,
    key: i64,
    /// The connection holding the lock. `None` means this replica is a follower.
    held: Held,
    /// The same fact, readable without awaiting, for status answers.
    leader: Arc<AtomicBool>,
    /// The server-side lease on the lock session.
    lease: Duration,
    /// The task keeping the lock session busy while this replica leads.
    heartbeat: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Leadership {
    /// A claim on `key` in the database behind `pool`.
    pub fn new(pool: PgPool, key: i64) -> Self {
        Self::with_lease(pool, key, LEASE)
    }

    /// A claim whose lock the server drops `lease` after the leader last spoke.
    pub fn with_lease(pool: PgPool, key: i64, lease: Duration) -> Self {
        Self {
            pool,
            key,
            held: Arc::new(Mutex::new(None)),
            leader: Arc::new(AtomicBool::new(false)),
            lease,
            heartbeat: std::sync::Mutex::new(None),
        }
    }

    /// A claim on the reconciler lock.
    pub fn reconciler(pool: PgPool) -> Self {
        Self::new(pool, RECONCILER_LOCK_KEY)
    }

    /// Whether this replica currently holds the lock, as of the last [`Leadership::acquire`].
    pub fn is_leader(&self) -> bool {
        self.leader.load(Ordering::Relaxed)
    }

    /// Takes the lock, or confirms that this replica still holds it.
    ///
    /// Call it before every reconcile rather than once at startup: a lock held by a session
    /// that has since died is not held at all, and only asking the database says so. Losing
    /// the connection demotes this replica instead of failing the call, because a follower is
    /// a correct state and a hard error here would stop the loop that recovers from it.
    pub async fn acquire(&self) -> Result<bool, sqlx::Error> {
        let mut held = self.held.lock().await;

        if let Some(connection) = held.as_mut() {
            match sqlx::query("SELECT 1").execute(&mut **connection).await {
                Ok(_) => return Ok(true),
                Err(err) => {
                    tracing::warn!(error = %err, "the connection holding the reconciler lock died");
                    if let Some(connection) = held.take() {
                        close(connection).await;
                    }
                    self.leader.store(false, Ordering::Relaxed);
                }
            }
        }

        let mut connection = self.pool.acquire().await?;
        let won: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(self.key)
            .fetch_one(&mut *connection)
            .await?;

        if won {
            // The lease is set only on a session that holds the lock, and that session never
            // goes back to the pool (see `close`), so no other query inherits the timeout.
            let lease = format!("{}ms", self.lease.as_millis());
            if let Err(err) = sqlx::query("SELECT set_config('idle_session_timeout', $1, false)")
                .bind(&lease)
                .execute(&mut *connection)
                .await
            {
                tracing::warn!(error = %err, "the reconciler lock needs PostgreSQL 14 or later for its lease (idle_session_timeout); not leading");
                close(connection).await;
                self.leader.store(false, Ordering::Relaxed);
                return Err(err);
            }
            *held = Some(connection);
            self.start_heartbeat();
        }
        self.leader.store(won, Ordering::Relaxed);
        Ok(won)
    }

    /// Keeps the lock session busy, so the server's lease never ends it while this replica
    /// lives; a beat that fails demotes this replica at once instead of at the next reconcile.
    fn start_heartbeat(&self) {
        let (held, leader) = (Arc::clone(&self.held), Arc::clone(&self.leader));
        let period = self.lease / 3;
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(period).await;
                let mut guard = held.lock().await;
                let Some(connection) = guard.as_mut() else {
                    return;
                };
                if let Err(err) = sqlx::query("SELECT 1").execute(&mut **connection).await {
                    tracing::warn!(error = %err, "the reconciler lock's heartbeat failed; this replica stops leading");
                    if let Some(connection) = guard.take() {
                        close(connection).await;
                    }
                    leader.store(false, Ordering::Relaxed);
                    return;
                }
            }
        });
        if let Some(previous) = self
            .heartbeat
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .replace(task)
        {
            previous.abort();
        }
    }

    fn stop_heartbeat(&self) {
        if let Some(task) = self
            .heartbeat
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            task.abort();
        }
    }

    /// Gives the lock up, so another replica can take it without waiting for this one to die.
    ///
    /// The session ends with it: an advisory lock belongs to the session, and a pooled session
    /// handed to the next caller would still hold the lock and carry the lease.
    pub async fn resign(&self) {
        self.stop_heartbeat();
        let mut held = self.held.lock().await;
        self.leader.store(false, Ordering::Relaxed);
        if let Some(mut connection) = held.take() {
            // Unlock first: the server frees a closed session's locks only when its backend has
            // exited, which on a busy server is after `resign` returned (T-2977). The close
            // still keeps the session and its lease out of the pool.
            if let Err(err) = sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(self.key)
                .execute(&mut *connection)
                .await
            {
                tracing::warn!(error = %err, "could not release the reconciler lock explicitly");
            }
            close(connection).await;
        }
    }
}

impl Drop for Leadership {
    fn drop(&mut self) {
        self.stop_heartbeat();
    }
}

/// Ends a lock session instead of pooling it; ending the session releases the lock.
async fn close(connection: PoolConnection<Postgres>) {
    if let Err(err) = connection.detach().close().await {
        // The socket is gone either way; the server ends the session when it notices.
        tracing::warn!(error = %err, "the reconciler lock session did not close cleanly");
    }
}

impl std::fmt::Debug for Leadership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Leadership")
            .field("key", &self.key)
            .field("leader", &self.is_leader())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_key_is_a_readable_constant() {
        // ASCII, so the key stays positive and readable where `pg_locks` shows it.
        assert_eq!(RECONCILER_LOCK_KEY.to_be_bytes(), *b"jc_recon");
    }

    fn database_url() -> Option<String> {
        std::env::var("JC_PORTAL_TEST_DATABASE_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
    }

    /// A key of this test alone, so parallel tests on one database never share a lock.
    fn private_key(n: i64) -> i64 {
        0x6a63_7465_7374_0000 + n
    }

    const SHORT: Duration = Duration::from_millis(1200);

    /// A leader that goes silent without closing its session, like a pod killed behind its
    /// proxy, must not keep the lock: the next replica leads once the lease is over.
    #[tokio::test]
    async fn a_silent_leader_loses_the_lock_after_its_lease() {
        let Some(url) = database_url() else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect and migrate");
        let key = private_key(1);
        let silent = Leadership::with_lease(pool.clone(), key, SHORT);
        let next = Leadership::with_lease(pool.clone(), key, SHORT);

        assert!(silent.acquire().await.expect("first election"));
        assert!(!next.acquire().await.expect("second election"));

        // The process freezes: no heartbeat, no FIN, the session stays open on the server.
        silent.stop_heartbeat();
        tokio::time::sleep(SHORT * 2).await;

        assert!(
            next.acquire().await.expect("election after the lease"),
            "the lock of a silent leader must be free one lease later"
        );
        next.resign().await;
    }

    /// A live leader keeps the lock across many leases without reconciling in between.
    #[tokio::test]
    async fn a_beating_leader_keeps_the_lock_past_its_lease() {
        let Some(url) = database_url() else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect and migrate");
        let key = private_key(2);
        let leader = Leadership::with_lease(pool.clone(), key, SHORT);
        let other = Leadership::with_lease(pool.clone(), key, SHORT);

        assert!(leader.acquire().await.expect("election"));
        tokio::time::sleep(SHORT * 3).await;

        assert!(leader.is_leader());
        assert!(
            !other.acquire().await.expect("second election"),
            "the heartbeat must keep a live leader's lock"
        );
        assert!(leader.acquire().await.expect("re-election"));

        leader.resign().await;
        assert!(
            other.acquire().await.expect("election after resignation"),
            "resigning frees the lock at once"
        );
        other.resign().await;
    }

    /// Resigning frees the lock before it returns: another replica asking right after it wins,
    /// every time, not only once the closed session's backend has gone (T-2977).
    #[tokio::test]
    async fn the_lock_is_free_the_moment_resign_returns() {
        let Some(url) = database_url() else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect and migrate");
        let key = private_key(4);
        let (one, other) = (
            Leadership::with_lease(pool.clone(), key, SHORT),
            Leadership::with_lease(pool.clone(), key, SHORT),
        );
        for round in 0..50 {
            let (leader, next) = if round % 2 == 0 {
                (&one, &other)
            } else {
                (&other, &one)
            };
            assert!(leader.acquire().await.expect("election"), "round {round}");
            leader.resign().await;
            assert!(
                next.acquire().await.expect("election after resignation"),
                "round {round}: the lock was still held after resign returned"
            );
            next.resign().await;
        }
    }

    /// The lease never leaks into the pool: after a resignation the pool's sessions carry the
    /// server's default idle_session_timeout.
    #[tokio::test]
    async fn a_resigned_lock_session_is_not_pooled_with_its_lease() {
        let Some(url) = database_url() else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect and migrate");
        let leader = Leadership::with_lease(pool.clone(), private_key(3), SHORT);
        assert!(leader.acquire().await.expect("election"));
        leader.resign().await;

        let mut sessions = Vec::new();
        for _ in 0..pool.size().max(1) {
            sessions.push(pool.acquire().await.expect("pooled session"));
        }
        for session in &mut sessions {
            let timeout: String = sqlx::query_scalar("SHOW idle_session_timeout")
                .fetch_one(&mut **session)
                .await
                .expect("show");
            assert_eq!(
                timeout, "0",
                "a pooled session must not carry the lock's lease"
            );
        }
    }
}
