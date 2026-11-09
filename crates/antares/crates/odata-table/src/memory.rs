// SPDX-License-Identifier: MIT
//! [`MemoryTable`]: rows held in memory, evaluated by the reference semantics.

use std::collections::HashSet;

use serde_json::Value;

use crate::eval::{matches, ordered};
use crate::{Fetched, Fetching, Plan, Scope, Table, TableError, TableInfo};

/// An entity set over rows in memory: JSON objects, each with a unique string or integer key.
#[derive(Debug, Clone)]
pub struct MemoryTable {
    info: TableInfo,
    rows: Vec<Value>,
}

impl MemoryTable {
    /// The set `info` over `rows`.
    ///
    /// # Errors
    ///
    /// [`TableError::BadRequest`] when a row is not an object, lacks a string or integer key,
    /// or repeats another row's key.
    pub fn new(info: TableInfo, rows: Vec<Value>) -> Result<Self, TableError> {
        let mut seen = HashSet::new();
        for row in &rows {
            let key = match row.get(&info.key) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) if n.is_i64() || n.is_u64() => n.to_string(),
                _ => {
                    return Err(TableError::BadRequest(format!(
                        "every row of {} needs a string or integer {}",
                        info.set, info.key
                    )))
                }
            };
            if !seen.insert(key) {
                return Err(TableError::BadRequest(format!(
                    "two rows of {} share one {}",
                    info.set, info.key
                )));
            }
        }
        Ok(Self { info, rows })
    }

    /// The rows of `plan` within `scope`, synchronously.
    #[must_use]
    pub fn answer(&self, plan: &Plan, scope: &Scope) -> Fetched {
        let scope = scope.cond();
        // the scope first: the filter only ever sees rows the caller may read
        let mut kept: Vec<&Value> = self
            .rows
            .iter()
            .filter(|row| matches(row, &scope))
            .filter(|row| matches(row, &plan.filter))
            .collect();
        kept.sort_by(|a, b| ordered(a, b, &plan.order));
        let count = plan
            .count
            .then(|| u64::try_from(kept.len()).unwrap_or(u64::MAX));
        let skip = usize::try_from(plan.offset).unwrap_or(usize::MAX);
        let take = usize::try_from(plan.fetch).unwrap_or(usize::MAX);
        Fetched {
            rows: kept.into_iter().skip(skip).take(take).cloned().collect(),
            count,
        }
    }
}

impl Table for MemoryTable {
    fn info(&self) -> &TableInfo {
        &self.info
    }

    fn fetch<'a>(&'a self, plan: &'a Plan, scope: &'a Scope) -> Fetching<'a> {
        Box::pin(std::future::ready(Ok(self.answer(plan, scope))))
    }
}
