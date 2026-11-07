// SPDX-License-Identifier: EUPL-1.2
//! A collection's Features, read as current state ([`items`]) or as its `{type}-history`
//! ([`history`], T-5100). The one entry point keeps the two readers one-way: history builds on
//! the items helpers, items knows nothing of history.

use crate::items::{Page, Params, Refs};
use crate::model::Collection;
use crate::{history, items, Problem};
use antares_ops::{Caller, ViewOps};
use serde_json::{Map, Value};

/// One page of the collection's Features, see [`items::page`] and [`history::page`].
pub(crate) async fn page(
    view: &ViewOps,
    caller: &Caller,
    c: &Collection,
    p: &Params,
    digest: u64,
    refs: Refs<'_>,
) -> Result<Page, Problem> {
    if c.history {
        history::page(view, caller, c, p, digest, refs).await
    } else {
        items::page(view, caller, c, p, digest, refs).await
    }
}

/// One Feature by its id, see [`items::one`] and [`history::one`].
pub(crate) async fn one(
    view: &ViewOps,
    caller: &Caller,
    c: &Collection,
    id: &str,
    p: &Params,
    refs: Refs<'_>,
) -> Result<Map<String, Value>, Problem> {
    if c.history {
        history::one(view, caller, c, id, p, refs).await
    } else {
        items::one(view, caller, c, id, p, refs).await
    }
}
