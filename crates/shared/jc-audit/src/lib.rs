#![forbid(unsafe_code)]
//! `jc-audit`: an audit row commits in the same transaction as the change it records, and a
//! change whose row cannot be written does not stand (AUD-02, T-7565).
//!
//! A module's door runs a write request inside a unit ([`layer`] for an axum router, [`scope`]
//! elsewhere). Inside a unit, the module's store opens its transactions with [`begin`] and ends
//! them with [`commit`]: the first call takes one pooled connection and sends `BEGIN`; every call
//! then runs inside a savepoint of that one transaction, so a store call that returns early or
//! fails undoes only its own statements, exactly as its own transaction did before. The unit
//! commits once at the end, when the request succeeded, and rolls back otherwise. A store that
//! writes its audit row through the same [`begin`] therefore commits change and row together.
//!
//! Outside a unit [`begin`] and [`commit`] are a plain `pool.begin()` and `tx.commit()`.
//! A task spawned from a request does not inherit the unit (a tokio task-local) and runs its own
//! transactions. ponytail: one unit holds one connection of one pool; a door that writes to two
//! modules' pools needs a unit per pool.

use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sqlx::pool::PoolConnection;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, OwnedMappedMutexGuard, OwnedMutexGuard};

/// How long the unit's connection may sit idle inside its open transaction before Postgres ends
/// the session; the unit never waits on anything but its own request.
const IDLE_IN_TRANSACTION: &str = "SET LOCAL idle_in_transaction_session_timeout = '30s'";

/// The connection of a unit with its transaction still open. Dropped while open (a cancelled
/// request, a failed `ROLLBACK`), it is closed instead of going back to the pool mid-transaction.
struct Held {
    conn: PoolConnection<Postgres>,
    open: bool,
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.open {
            self.conn.close_on_drop();
        }
    }
}

#[derive(Clone, Default)]
struct Unit {
    conn: Arc<Mutex<Option<Held>>>,
    /// A store call ended without [`commit`]: its savepoint is still to roll back.
    dirty: Arc<AtomicBool>,
    /// The door marked the request failed ([`abort`]) although it answers with a success status.
    aborted: Arc<AtomicBool>,
    /// Work that may run only once what the unit wrote stands ([`after_commit`]).
    after: Arc<std::sync::Mutex<Vec<AfterCommit>>>,
}

type AfterCommit = Pin<Box<dyn Future<Output = ()> + Send>>;

tokio::task_local! {
    static UNIT: Unit;
}

/// Marks a store call's savepoint for rollback unless [`commit`] disarmed it.
struct Savepoint {
    dirty: Arc<AtomicBool>,
    armed: bool,
}

impl Drop for Savepoint {
    fn drop(&mut self) {
        if self.armed {
            self.dirty.store(true, Ordering::SeqCst);
        }
    }
}

enum Inner {
    Own(Transaction<'static, Postgres>),
    Unit {
        conn: OwnedMappedMutexGuard<Option<Held>, PoolConnection<Postgres>>,
        savepoint: Savepoint,
    },
}

/// A store call's transaction: its own outside a unit, a savepoint of the unit's inside one.
/// Dereferences to the connection, so `&mut *tx` is an executor as with a sqlx transaction.
pub struct Tx(Inner);

impl Deref for Tx {
    type Target = PgConnection;
    fn deref(&self) -> &PgConnection {
        match &self.0 {
            Inner::Own(tx) => tx,
            Inner::Unit { conn, .. } => conn,
        }
    }
}

impl DerefMut for Tx {
    fn deref_mut(&mut self) -> &mut PgConnection {
        match &mut self.0 {
            Inner::Own(tx) => tx,
            Inner::Unit { conn, .. } => conn,
        }
    }
}

async fn exec(conn: &mut PgConnection, sql: &'static str) -> Result<(), sqlx::Error> {
    sqlx::query(sql).execute(conn).await.map(drop)
}

/// Rolls back the savepoint a store call left behind, so the next call starts clean even after
/// a failed statement put the transaction in the aborted state.
async fn undo_dirty(conn: &mut PgConnection, dirty: &AtomicBool) -> Result<(), sqlx::Error> {
    if dirty.swap(false, Ordering::SeqCst) {
        exec(conn, "ROLLBACK TO SAVEPOINT jc_call").await?;
        exec(conn, "RELEASE SAVEPOINT jc_call").await?;
    }
    Ok(())
}

/// Opens a store call's transaction on `pool`: inside a unit a savepoint of the unit's one
/// transaction (opened on the first call), outside a unit a transaction of its own. Calls of one
/// unit made at the same time wait for each other.
pub async fn begin(pool: &PgPool) -> Result<Tx, sqlx::Error> {
    let Ok(unit) = UNIT.try_with(Unit::clone) else {
        return Ok(Tx(Inner::Own(pool.begin().await?)));
    };
    let mut guard: OwnedMutexGuard<Option<Held>> = unit.conn.lock_owned().await;
    if guard.is_none() {
        let mut held = Held {
            conn: pool.acquire().await?,
            open: true,
        };
        exec(&mut held.conn, "BEGIN").await?;
        exec(&mut held.conn, IDLE_IN_TRANSACTION).await?;
        *guard = Some(held);
    }
    let Ok(mut conn) = OwnedMutexGuard::try_map(guard, |held| held.as_mut().map(|h| &mut h.conn))
    else {
        return Err(sqlx::Error::PoolClosed);
    };
    undo_dirty(&mut conn, &unit.dirty).await?;
    exec(&mut conn, "SAVEPOINT jc_call").await?;
    Ok(Tx(Inner::Unit {
        conn,
        savepoint: Savepoint {
            dirty: unit.dirty,
            armed: true,
        },
    }))
}

/// Opens a transaction of its own even inside a unit, for a write that must stand whatever the
/// request's outcome: a replay guard (a used DPoP proof, a spent challenge) rolled back with a
/// failed request would accept the same proof again.
pub async fn begin_own(pool: &PgPool) -> Result<Tx, sqlx::Error> {
    Ok(Tx(Inner::Own(pool.begin().await?)))
}

/// Ends a store call's transaction: commits it outside a unit, releases its savepoint inside one
/// (the unit commits at its end). A `Tx` dropped without this rolls back its own statements.
pub async fn commit(tx: Tx) -> Result<(), sqlx::Error> {
    match tx.0 {
        Inner::Own(tx) => tx.commit().await,
        Inner::Unit {
            mut conn,
            mut savepoint,
        } => {
            exec(&mut conn, "RELEASE SAVEPOINT jc_call").await?;
            savepoint.armed = false;
            Ok(())
        }
    }
}

/// Marks the current request failed, so its unit rolls back although the answer's status is a
/// success (a JSON-RPC error inside a 200). Outside a unit it does nothing.
pub fn abort() {
    let _ = UNIT.try_with(|u| u.aborted.store(true, Ordering::SeqCst));
}

/// Runs `fut` once what the request wrote stands: inside a unit after its commit (never when it
/// rolls back), outside a unit at once. For work another party sees, such as a webhook sent
/// about the change, or a cache marked from it. It runs outside the unit, on the task that ends
/// it, so its own store calls open transactions of their own.
pub async fn after_commit<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let mut fut = Some(fut);
    let _ = UNIT.try_with(|u| {
        if let Some(f) = fut.take() {
            u.after
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Box::pin(f));
        }
    });
    if let Some(f) = fut {
        f.await;
    }
}

async fn run_after(after: Vec<AfterCommit>) {
    for f in after {
        f.await;
    }
}

/// Why a unit did not commit what its request wrote.
#[derive(Debug)]
pub enum UnitError {
    /// The request ended successfully, but the transaction did not commit: nothing it wrote stands.
    Commit(sqlx::Error),
}

impl std::fmt::Display for UnitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Commit(e) => write!(f, "the change was not saved: {e}"),
        }
    }
}

impl std::error::Error for UnitError {}

/// Runs `fut` as one unit. When `succeeded` says yes for its output (and nothing called
/// [`abort`]), every store call of it commits together; otherwise all of them roll back.
/// The commit runs on its own task, so a caller that goes away cannot leave it half done.
pub async fn scope<F, T>(succeeded: impl FnOnce(&T) -> bool, fut: F) -> Result<T, UnitError>
where
    F: Future<Output = T>,
{
    let unit = Unit::default();
    let out = UNIT.scope(unit.clone(), fut).await;
    let commit = succeeded(&out) && !unit.aborted.load(Ordering::SeqCst);
    let after = std::mem::take(
        &mut *unit
            .after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    let after = if commit { after } else { Vec::new() };
    let Some(mut held) = unit.conn.lock().await.take() else {
        // Nothing was written through the pool: the work after the commit is due now.
        let _ = tokio::spawn(run_after(after)).await;
        return Ok(out);
    };
    let dirty = unit.dirty;
    let ended = tokio::spawn(async move {
        let result = async {
            undo_dirty(&mut held.conn, &dirty).await?;
            exec(&mut held.conn, if commit { "COMMIT" } else { "ROLLBACK" }).await
        }
        .await;
        held.open = result.is_err();
        drop(held);
        if result.is_ok() {
            run_after(after).await;
        }
        result
    })
    .await;
    match ended {
        Ok(Ok(())) => Ok(out),
        Ok(Err(e)) if commit => Err(UnitError::Commit(e)),
        Err(e) if commit => Err(UnitError::Commit(sqlx::Error::Io(std::io::Error::other(e)))),
        // A rollback that failed closed its connection: the request already failed.
        _ => Ok(out),
    }
}

/// Axum middleware: runs every request that may write (any method but GET, HEAD and OPTIONS)
/// as one unit, committed when the answer is a 2xx. A commit that fails answers 500, since the
/// handler's answer would claim a change that was not saved.
pub async fn layer(req: Request, next: Next) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }
    match scope(|resp: &Response| resp.status().is_success(), next.run(req)).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, "a write request's transaction did not commit");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                r#"{"error":"server_error","error_description":"the change was not saved; try again"}"#,
            )
                .into_response()
        }
    }
}
