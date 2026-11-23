//! Where every sqlx pool of jc starts (T-7499; `tests/call_sites.rs` refuses another start).

use sqlx_core::database::Database;
use sqlx_core::pool::PoolOptions;
use std::time::Duration;

/// How long a connection sits idle before its pool closes it (T-6201). The pool looks once per
/// this time, so a healthy idle session is gone within twice it: what the watch row
/// `database.held` (bins/jc/smoke/orgs) takes a session idle for longer by.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Pool options with sqlx's defaults and [`IDLE_TIMEOUT`].
///
/// A released connection that fails sqlx's test is closed and its slot given back: sqlx-core
/// 0.9.0 waited, closing over TLS, for the idle server to send something and never gave the slot
/// back (T-7494, jc-prod 2026-11-22); 0.9.1 only writes out its last records (launchbadge/sqlx
/// #4449), so jc carries no release hook of its own.
pub fn options<DB: Database>() -> PoolOptions<DB> {
    PoolOptions::new().idle_timeout(IDLE_TIMEOUT)
}
