// SPDX-License-Identifier: EUPL-1.2
//! `antares-sta` against a broker: one module for the entity sets, one for the bounds and the
//! refusals (docs/analysis/sta-mapping.md), one for the read path prod failed on (T-4928), sharing
//! one fixture.

mod bounds;
mod common;
mod findings;
mod navigation;
mod outputs;
mod paging;
mod sets;
