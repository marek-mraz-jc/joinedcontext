// SPDX-License-Identifier: EUPL-1.2
//! /temporal/entities (5.6.11–5.6.16, 5.7.3/5.7.4; resources 6.18–6.22).

use crate::negotiate::*;
use crate::state::{now_iso, AppState};
use crate::temporalq::TemporalQ;
use antares_jsonld::compact::compact_instance;
use antares_jsonld::{expand_entity, Context, ExpandOpts};
use antares_model::{dt_key, NgsiError, TenantId};
use antares_ql::parse_q;
use antares_store::TemporalDriverExt as _;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

use crate::negotiate::CleanParams;

use antares_model::is_meta;

mod aggregate;
mod filter;
mod query;
mod render;
mod retrieve;
#[cfg(test)]
mod tests;
mod write;

use aggregate::*;
pub(crate) use filter::*;
pub use query::*;
pub(crate) use render::*;
pub use retrieve::*;
pub use write::*;

// ---------- temporal query params (4.11) ----------

/// Windowed per-entity temporal data: filtered+ordered instances per attr.
struct Windowed {
    attrs: std::collections::BTreeMap<String, Vec<Value>>,
    max_per_attr: usize,
    ts_min: Option<String>,
    ts_max: Option<String>,
    truncated: bool,
    /// some Attribute holds instances past this page in its order: the
    /// response links to the next page (6.4.7.3)
    more: bool,
    /// the fewest instances an Attribute the ceiling cut shortened still
    /// served (`truncate`): how far the next offsetN page may move
    cut_served: Option<usize>,
    /// Attributes their type records `onChange` (ADR-N-054 2.5): each value
    /// holds until the next row, so a window starts at the level carried in
    /// and an aggregation weighs a value by the time it held
    held: std::collections::HashSet<String>,
}

/// TS 104 176 6.4.7.3: which instances of each Attribute one response
/// carries. firstN pages oldest first, lastN newest first, neither oldest
/// first; offsetN skips that many in the chosen order.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub(crate) struct InstancePage {
    /// lastN: newest first
    desc: bool,
    /// the firstN or lastN asked for
    n: Option<usize>,
    /// offsetN
    offset: usize,
}

impl InstancePage {
    /// firstN, lastN and offsetN, validated: N positive (5.2.21), offsetN
    /// not negative (Table 6.4.7.3-2), both at most i64::MAX (bound as a
    /// `bigint` they would wrap negative and silently select nothing).
    /// firstN asks for ascending order and lastN for descending; one page
    /// cannot be both, so the pair is BadRequestData.
    fn parse(params: &HashMap<String, String>) -> Result<Self, NgsiError> {
        let count = |name: &str, min: usize| -> Result<Option<usize>, NgsiError> {
            params
                .get(name)
                .map(|v| {
                    v.parse::<usize>()
                        .ok()
                        .filter(|n| *n >= min && *n <= i64::MAX as usize)
                        .ok_or_else(|| NgsiError::BadRequestData(format!("invalid {name} {v:?}")))
                })
                .transpose()
        };
        let (first, last) = (count("firstN", 1)?, count("lastN", 1)?);
        if first.is_some() && last.is_some() {
            return Err(NgsiError::BadRequestData(
                "firstN pages ascending and lastN descending: give one of them (6.4.7.3)".into(),
            ));
        }
        Ok(Self {
            desc: last.is_some(),
            n: first.or(last),
            offset: count("offsetN", 0)?.unwrap_or(0),
        })
    }

    /// The most instances of one Attribute a page holds: N, or the broker's
    /// ceiling when N is above it (6.4.7.3 "whichever is smaller"); with no
    /// N, the broker's default, at most the ceiling (T-4804).
    fn size(&self, default: usize, limit: usize) -> usize {
        self.n.map_or(default.min(limit), |n| n.min(limit))
    }

    /// How far one page moves offsetN: its size, or N itself for an
    /// aggregated representation, which the ceiling never cuts.
    fn step(&self, aggregated: bool, default: usize, limit: usize) -> usize {
        match (aggregated, self.n) {
            (true, Some(n)) => n,
            _ => self.size(default, limit),
        }
    }

    /// The per-Attribute cap a store may apply before the window: every
    /// instance up to the end of this page in its order, and one more, so
    /// the window still sees whether a next page exists. Without N the page
    /// is `default` instances, the broker's default page, when the caller
    /// pages instances at all (`None` for an aggregation, which reads the
    /// whole window): a chart's plain window read paged by offsetN used to
    /// rebuild every instance of the window per page (T-5777).
    fn store_cap(&self, default: Option<usize>) -> Option<i64> {
        self.n.or(default).map(|n| {
            n.saturating_add(self.offset)
                .saturating_add(1)
                .min(i64::MAX as usize) as i64
        })
    }
}

/// The page a read without firstN/lastN serves per Attribute, for the
/// store's cap: none for an aggregation (it reads the whole window) or a
/// tenant recording `onChange`, whose window starts at the level carried in.
fn default_page(st: &AppState, tenant: &TenantId, trepr: &TRepr) -> Option<usize> {
    (!trepr.aggregated && !crate::history::records_on_change(st, tenant)).then(|| {
        st.temporal_default_instances
            .min(st.temporal_instance_limit)
    })
}

/// Parsed temporal representation params (options/format/lastN/pick/omit/
/// datasetId/aggregation), fully validated up front.
#[derive(Default, Clone)]
pub(crate) struct TRepr {
    temporal_values: bool,
    aggregated: bool,
    sys: bool,
    page: InstancePage,
    pick: Option<Vec<crate::repr::ProjNode>>,
    omit: Option<Vec<crate::repr::ProjNode>>,
    dataset_id: Option<Vec<String>>,
    attrs: Option<Vec<String>>,
    aggr_methods: Vec<String>,
    aggr_period: AggrPeriod,
    lang: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum AggrPeriod {
    /// PT0S / absent: one bucket over the whole range
    #[default]
    Whole,
    Seconds(i64),
    /// 4.5.19.1: a period may mix date and time elements
    /// ("P3Y6M4DT12H30M5S"), so a month step carries the leftover seconds —
    /// months are not a fixed number of seconds and cannot be folded in.
    Months(u32, i64),
}

/// 5.6.13.4-5.6.15.4 shared forwarding: proxy registrations without the
/// operation's support are an error of type Conflict and are never
/// contacted; supporting registrations receive the forwarded request. None
/// = no matching registrations (the operation stays purely local).
/// `path_suffix` arrives with its client-controlled segments already
/// percent-encoded (RFC 3986 clause 3.3); the entity id is encoded here.
/// What the local `mutate` did, in the words the answer needs. 5.6.13,
/// 5.6.14, 5.6.15 and 5.6.16 all answer the same three ways: 204 when the
/// target was there, ResourceNotFound naming the Entity when the Entity was
/// not, and ResourceNotFound naming the Attribute or the instance when the
/// Entity was there but the target inside it was not.
struct LocalWrite {
    /// `None` when the Temporal Evolution itself is absent.
    res: Option<Result<(), NgsiError>>,
    /// whether the operation's target inside it was there.
    found: bool,
    /// what a 404 names once the Entity itself was found.
    missing: String,
    /// what the 204 Part reports was done here.
    applied: &'static str,
}
