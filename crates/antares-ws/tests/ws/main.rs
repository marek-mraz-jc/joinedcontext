// SPDX-License-Identifier: EUPL-1.2
//! ADR-0026's Confirmation table, one module per rule, over one broker per
//! test on a real socket.

mod common;
mod ws_bounds;
mod ws_contract;
mod ws_delivery;
mod ws_hub;
mod ws_lease;
mod ws_protocol;
mod ws_queue;
mod ws_resume;
mod ws_seams;
mod ws_sta_mqtt;
mod ws_views;
