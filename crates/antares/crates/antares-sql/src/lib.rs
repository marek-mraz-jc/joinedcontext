// SPDX-License-Identifier: EUPL-1.2
//! AST → SQL compilation + schema.
//!
//! The q= compiler emits parameterized SQL — structure from the
//! compiler, values as binds ONLY. The sqlx store implementations live in
//! `store`; migrations live in `migrations/`.
// T-3468: every surface calls into this crate on a request path, where a
// panic is a remote DoS. Outside tests it may not unwrap, expect, panic,
// index out of bounds or overflow.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod compile;
/// Re-export for integration tests that need raw SQL against the pool.
#[cfg(feature = "postgres")]
pub use sqlx;
pub mod store;

pub use antares_store::StoreMode;

/// The transaction preamble that makes RLS effective: always SET LOCAL,
/// never session-level SET.
pub const SET_TENANT_SQL: &str = "SELECT set_config('antares.tenant', $1, true)";
