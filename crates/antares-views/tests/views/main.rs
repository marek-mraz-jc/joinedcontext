// SPDX-License-Identifier: EUPL-1.2
//! The view root's integration tests, one binary: the reads per ADR-0023
//! rule (`views_read`), the caller's `q` (`views_q`), snapshots
//! (`views_tenant`), the writes per rule 7 (`views_writes`), Subscriptions
//! and notifications per rule 9 (`views_subscriptions`) and what it
//! answers when it does not serve (`views_errors`), over the fixture in
//! `common`.

mod common;
mod views_all;
mod views_decision;
mod views_errors;
mod views_mapped;
mod views_mapped_props;
mod views_mapped_reads;
mod views_maps;
mod views_q;
mod views_read;
mod views_routing;
mod views_subscriptions;
mod views_tenant;
mod views_union;
mod views_writes;
