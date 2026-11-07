// SPDX-License-Identifier: EUPL-1.2
//! JSON-LD layer — hand-rolled NGSI-LD-subset processor: context processing,
//! expansion with structural validation, compaction, caching loader with
//! pinned core contexts.
//!
#![doc = include_str!("../README.md")]
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
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod compact;
pub mod context;
pub mod expand;
#[cfg(feature = "fetch")]
pub mod loader;
// compiled for the native tests too, so its bounds are tested where CI runs
#[cfg(any(test, all(feature = "fetch", target_arch = "wasm32")))]
pub(crate) mod minicache;
pub mod pinned;

pub use compact::{compact_entity, compact_entity_shallow, compact_types};
pub use context::{Context, DEFAULT_VOCAB, NGSI_LD_BASE};
pub use expand::{
    expand_attr_fragment, expand_attr_name, expand_entity, expand_types, expanded_id,
    expanded_object, is_deletion_instance, is_ngsi_null, is_ngsi_null_langmap, is_ngsi_null_list,
    parse_datetime, reject_first_level_nulls, removes_member, valid_name, valid_scope_value,
    ExpandOpts, RESERVED_MEMBERS,
};
#[cfg(feature = "fetch")]
pub use loader::{
    allow_private_egress, client_builder, http_interaction, install_crypto_provider, io_deadline,
    slow_factor, with_timeouts, wrap_client, CtxUsage, EgressPolicy, HttpClient, Loader,
    INTERNAL_FETCH_HEADER, MAX_REDIRECTS,
};
pub use pinned::{
    core_context, core_context_url, resolve_with, ContextLoader, PinnedOnly, CORE_CONTEXT,
    MAX_CONTEXT_URLS,
};
