//! Where every sqlx pool of jc starts (T-7499; `tests/call_sites.rs` refuses another start).

use sqlx_core::connection::Connection;
use sqlx_core::database::Database;
use sqlx_core::pool::PoolOptions;
use std::time::Duration;

/// How long a connection sits idle before its pool closes it (T-6201). The pool looks once per
/// this time, so a healthy idle session is gone within twice it: what the watch row
/// `database.held` (bins/jc/smoke/orgs) takes a session idle for longer by.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Pool options with sqlx's defaults, [`IDLE_TIMEOUT`] and the release rule below.
///
/// A released connection is tested here before sqlx tests it (T-7494). A future dropped
/// mid-statement leaves that statement's error for the test to read, and sqlx 0.9.0 answers a
/// failed test with `close_hard`, which over TLS first waits for the idle server to send
/// something: the slot is never given back. `Ok(false)` closes with the driver's goodbye, which
/// the server answers by hanging up, so a hook must never return `Err`. A pool that sets its own
/// `after_release` replaces this one and keeps that rule.
/// ponytail: one extra round trip per release; drop the hook with sqlx-core 0.9.1 (#4450).
pub fn options<DB: Database>() -> PoolOptions<DB> {
    PoolOptions::new()
        .idle_timeout(IDLE_TIMEOUT)
        .after_release(|connection: &mut DB::Connection, _| {
            Box::pin(async move {
                match connection.ping().await {
                    Ok(()) => Ok(true),
                    Err(error) => {
                        tracing::warn!(%error, "a released database connection is closed, not reused");
                        Ok(false)
                    }
                }
            })
        })
}
