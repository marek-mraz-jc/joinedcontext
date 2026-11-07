// SPDX-License-Identifier: EUPL-1.2
//! [`ViewOps::scan`]: a standard's query as the store's pushdown and the residual the store
//! cannot answer (ADR-0028 rules 3 to 5). The residual runs only on what [`ViewOps::run`]
//! answered for the caller, so it drops Entities and never adds one; a page is filled from an
//! upstream cursor within [`RESIDUAL_SCAN`]; a count or a residual sort reads the whole answer
//! when it fits the budget and is refused before any evaluation when it does not.

use axum::http::Method;
use axum::response::Response;
use serde_json::Value;

use crate::residual::{Program, Truth};
use crate::{Caller, OperationId, OpsRequest, Refusal, ViewOps};

/// The most upstream Entities one [`ViewOps::scan`] reads (ADR-0028 rule 4).
// ponytail: one constant for every deployment; an environment variable (documented in
// docs/src/configuration.md) when a deployment measures that it needs another value.
pub const RESIDUAL_SCAN: usize = 10_000;

/// The Query Entities the store answers, as a surface built it.
#[derive(Debug, Clone, Default)]
pub struct Pushdown {
    /// The encoded query parameters (`type=…&q=…`), without `limit`, `offset`, `count`,
    /// `orderBy`, `attrs` and `options=sysAttrs`: [`ViewOps::scan`] sets those.
    pub query: String,
    /// The `orderBy` keys the store applies (TS 104 176 7.2.3.1); with a residual the scan ends
    /// them with `id`, so the upstream order is total.
    pub order: Vec<String>,
    /// The Attributes the standard selects; `None` selects every one. The scan projects with
    /// `pick` (TS 104 175 4.21), never `attrs`, which would also drop the Entities holding none.
    pub attrs: Option<Vec<String>>,
    /// Whether the standard asks for `createdAt` and `modifiedAt`.
    pub sys_attrs: bool,
    /// The @context the names are read under ([`OpsRequest::context`]).
    pub context: Option<Value>,
}

/// Which part of the answer a [`ViewOps::scan`] serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// The upstream offset to read from: 0, or a previous page's `next`.
    pub start: usize,
    /// How many Entities the page holds at most.
    pub size: usize,
    /// The whole answer, counted, for a count or a sort the store cannot apply: `start` and
    /// `size` are then the surface's to apply after its own sort.
    pub whole: bool,
}

/// What a [`ViewOps::scan`] answered.
#[derive(Debug)]
pub enum Scanned {
    /// Entities the residual admitted, in upstream order.
    Page {
        /// The Entities, each as the standard selected it.
        entities: Vec<Value>,
        /// The upstream offset just past the last Entity served, when more may remain.
        next: Option<usize>,
        /// The exact count of the answer, when the page asked for the whole of it.
        count: Option<usize>,
        /// An upstream page said its join walk stopped at the lookup budget (an `NGSILD-Warning`
        /// naming `truncated`, TS 104 175 4.5.23.1): some Linked Entity is missing.
        truncated: bool,
    },
    /// The store's own answer to the pushdown when it was no success (a broken `q`: 400), for
    /// the surface to render as it renders [`ViewOps::run`]'s.
    Answer(Response),
}

/// Why a filter needs more of the store than one request may read.
pub const TOO_COMPLEX: &str = "the filter is larger than the residual evaluator allows";

impl ViewOps {
    /// Query Entities through the view with `residual` applied to what the store answered.
    ///
    /// Without a residual it is [`ViewOps::run`] of the pushdown with `limit`, `offset` and,
    /// for a whole answer, `count`, in upstream pages of the broker's `max_limit`. With one, the scan pushes `orderBy` ending in `id`,
    /// reads upstream pages of the broker's `max_limit` from `page.start` and keeps the
    /// Entities `residual` holds true for until the page is full, the answer ends or
    /// [`RESIDUAL_SCAN`] Entities were read; `next` is the offset just past the last one
    /// served. A whole answer reads `NGSILD-Results-Count` first and every Entity after it.
    /// The pushdown's `attrs` gains what the residual reads, and `options=sysAttrs` when it
    /// reads a system member; neither leaves the scan unless the standard selected it.
    ///
    /// # Errors
    /// [`Refusal::TooManyResults`] when a whole answer holds more than [`RESIDUAL_SCAN`]
    /// Entities, [`Refusal::Malformed`] when the residual's evaluation passes its step bound,
    /// and everything [`ViewOps::run`] refuses.
    pub async fn scan(
        &self,
        caller: &Caller,
        pushdown: &Pushdown,
        residual: Option<&Program>,
        page: Page,
    ) -> Result<Scanned, Refusal> {
        match residual {
            None => self.direct(caller, pushdown, page).await,
            Some(program) => {
                let mut budget = RESIDUAL_SCAN;
                self.scan_within(caller, pushdown, program, page, &mut budget)
                    .await
            }
        }
    }

    /// [`ViewOps::scan`] with a residual and at most `budget` upstream Entities read, less
    /// what it read: a surface reading one page in several scans carries the one
    /// [`RESIDUAL_SCAN`] budget across them (T-6036).
    ///
    /// # Errors
    /// As [`ViewOps::scan`], [`Refusal::TooManyResults`] against `budget`.
    pub async fn scan_within(
        &self,
        caller: &Caller,
        pushdown: &Pushdown,
        residual: &Program,
        page: Page,
        budget: &mut usize,
    ) -> Result<Scanned, Refusal> {
        let upstream = self.inner.state.max_limit;
        let (scanned, read) = self
            .fill(caller, pushdown, residual, page, (*budget, upstream))
            .await?;
        *budget = budget.saturating_sub(read);
        Ok(scanned)
    }

    /// The most Entities one upstream Query Entities answers (the broker's `max_limit`, at
    /// least 1): the page a surface streaming its answer reads at a time (T-6036).
    #[must_use]
    pub fn page_size(&self) -> usize {
        self.inner.state.max_limit.max(1)
    }

    /// The pushdown alone: [`ViewOps::run`] in upstream pages of the broker's `max_limit`
    /// until the page holds `page.size` or the answer ends; only the first asks for the count.
    async fn direct(
        &self,
        caller: &Caller,
        pushdown: &Pushdown,
        page: Page,
    ) -> Result<Scanned, Refusal> {
        let local = unpicked(pushdown, &[]);
        let fixed = format!(
            "{}{}",
            pushdown.query,
            params(pushdown, &pushdown.order, &[], false, local.is_none())
        );
        let upstream = self.inner.state.max_limit.max(1);
        let (mut entities, mut count, mut truncated) = (Vec::new(), None, false);
        let mut first = true;
        loop {
            let want = page.size.saturating_sub(entities.len());
            let limit = want.min(upstream);
            let counted = if page.whole && first {
                "&count=true"
            } else {
                ""
            };
            let offset = page.start.saturating_add(entities.len());
            let query = format!("{fixed}&limit={limit}&offset={offset}{counted}");
            let (mut batch, total, cut) =
                match read(self, self.query(caller, pushdown, &query).await?).await? {
                    Ok(read) => read,
                    Err(answer) => return Ok(Scanned::Answer(answer)),
                };
            if first {
                count = total.filter(|_| page.whole);
                first = false;
            }
            truncated |= cut;
            let ended = batch.len() < limit;
            for entity in &mut batch {
                strip(entity, &[], false, local);
            }
            entities.append(&mut batch);
            if ended || limit == want {
                let next = (!page.whole && !ended && limit > 0)
                    .then(|| page.start.saturating_add(entities.len()));
                return Ok(Scanned::Page {
                    entities,
                    next,
                    count,
                    truncated,
                });
            }
        }
    }

    /// The pushdown with `program` applied, upstream pages of `upstream` Entities and at most
    /// `budget` read, with how many it read.
    pub(crate) async fn fill(
        &self,
        caller: &Caller,
        pushdown: &Pushdown,
        program: &Program,
        page: Page,
        (budget, upstream): (usize, usize),
    ) -> Result<(Scanned, usize), Refusal> {
        let mut order = pushdown.order.clone();
        if order.last().map(String::as_str) != Some("id") {
            order.push("id".to_owned());
        }
        let extra: Vec<&str> = match &pushdown.attrs {
            Some(selected) => program
                .attributes()
                .into_iter()
                .filter(|a| !selected.iter().any(|s| s == a))
                .collect(),
            None => Vec::new(),
        };
        let system = program.reads_system_members() && !pushdown.sys_attrs;
        let local = unpicked(pushdown, &extra);
        let fixed = format!(
            "{}{}",
            pushdown.query,
            params(pushdown, &order, &extra, system, local.is_none())
        );
        // a whole answer reads what the count announced, a page what the budget allows
        let mut cap = budget;
        if page.whole {
            let counted = format!("{fixed}&limit=0&count=true");
            match read(self, self.query(caller, pushdown, &counted).await?).await? {
                Ok((_, total, _)) => {
                    // no count is no promise the answer fits
                    cap = total.unwrap_or(usize::MAX);
                    if cap > budget {
                        return Err(Refusal::TooManyResults { budget, size: cap });
                    }
                }
                Err(answer) => return Ok((Scanned::Answer(answer), 0)),
            }
        }
        let size = if page.whole { usize::MAX } else { page.size };
        let mut entities = Vec::new();
        let (mut offset, mut read_so_far, mut truncated) = (page.start, 0usize, false);
        if size == 0 {
            return Ok(served(entities, None, None, (read_so_far, truncated)));
        }
        loop {
            let limit = upstream.min(cap.saturating_sub(read_so_far));
            if limit == 0 {
                // A whole answer was read as counted; a page ran out of budget before it was
                // full and is served short with its continuation.
                let count = page.whole.then_some(entities.len());
                let next = (!page.whole).then_some(offset);
                return Ok(served(entities, next, count, (read_so_far, truncated)));
            }
            let query = format!("{fixed}&limit={limit}&offset={offset}");
            let batch = match read(self, self.query(caller, pushdown, &query).await?).await? {
                Ok((batch, _, cut)) => {
                    truncated |= cut;
                    batch
                }
                Err(answer) => return Ok((Scanned::Answer(answer), read_so_far)),
            };
            let last = batch.len();
            read_so_far = read_so_far.saturating_add(last);
            let ended = last < limit;
            for (i, mut entity) in batch.into_iter().enumerate() {
                offset = offset.saturating_add(1);
                match program.eval(&entity) {
                    Ok(Truth::True) => {}
                    Ok(_) => continue,
                    Err(_) => return Err(Refusal::Malformed(TOO_COMPLEX)),
                }
                strip(&mut entity, &extra, system, local);
                entities.push(entity);
                if entities.len() == size {
                    let more = !(ended && i.saturating_add(1) == last);
                    let next = more.then_some(offset);
                    return Ok(served(entities, next, None, (read_so_far, truncated)));
                }
            }
            if ended {
                let count = page.whole.then_some(entities.len());
                return Ok(served(entities, None, count, (read_so_far, truncated)));
            }
        }
    }

    async fn query(
        &self,
        caller: &Caller,
        pushdown: &Pushdown,
        query: &str,
    ) -> Result<Response, Refusal> {
        let mut req = OpsRequest::new(Method::GET, format!("/ngsi-ld/v1/entities?{query}"));
        if let Some(context) = &pushdown.context {
            req = req.context(context.clone());
        }
        self.run(OperationId::QueryEntities, caller, req).await
    }
}

/// A page of `entities` and the count of upstream Entities read for it.
fn served(
    entities: Vec<Value>,
    next: Option<usize>,
    count: Option<usize>,
    (read, truncated): (usize, bool),
) -> (Scanned, usize) {
    (
        Scanned::Page {
            entities,
            next,
            count,
            truncated,
        },
        read,
    )
}

/// `orderBy`, `pick` and `options` of a pushdown, each `&`-prefixed. `pick` keeps the
/// Entity's own members (4.21 drops what it does not name) and the system ones when read.
fn params(
    pushdown: &Pushdown,
    order: &[String],
    extra: &[&str],
    system: bool,
    pick: bool,
) -> String {
    let mut out = String::new();
    if !order.is_empty() {
        out.push_str("&orderBy=");
        out.push_str(&join(order.iter().map(String::as_str)));
    }
    let sys = pushdown.sys_attrs || system;
    if let Some(selected) = pushdown.attrs.as_ref().filter(|_| pick) {
        let own = ["id", "type", "scope"].into_iter().chain(
            sys.then_some(["createdAt", "modifiedAt"])
                .into_iter()
                .flatten(),
        );
        out.push_str("&pick=");
        out.push_str(&join(
            own.chain(selected.iter().map(String::as_str))
                .chain(extra.iter().copied()),
        ));
    }
    if sys {
        out.push_str("&options=sysAttrs");
    }
    out
}

/// The standard's selection when one of its names, or of those the residual adds, is not
/// [`pickable`]: nothing is projected upstream and the scan projects to it itself.
fn unpicked<'p>(pushdown: &'p Pushdown, extra: &[&str]) -> Option<&'p [String]> {
    pushdown.attrs.as_deref().filter(|selected| {
        !selected
            .iter()
            .map(String::as_str)
            .chain(extra.iter().copied())
            .all(pickable)
    })
}

/// Whether `pick` (TS 104 175 4.21) reads `name` as the one Attribute it is: a short name
/// without a dot (a dot is its sub-Attribute path) or an IRI (a `:` before any dot), in the
/// characters the projection grammar takes.
#[must_use]
pub fn pickable(name: &str) -> bool {
    let iri = name
        .find(':')
        .is_some_and(|colon| name.find('.').is_none_or(|dot| colon < dot));
    name.starts_with(|c: char| c.is_ascii_alphanumeric() || "_:@#".contains(c))
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_:#/%-+@".contains(c) || (c == '.' && iri))
}

/// Comma-joined and percent-encoded but for RFC 3986 unreserved characters.
fn join<'a>(names: impl Iterator<Item = &'a str>) -> String {
    names
        .map(crate::percent::encode)
        .collect::<Vec<_>>()
        .join(",")
}

/// The Entities of a successful Query Entities answer, its `NGSILD-Results-Count` and whether
/// it warns its join walk was truncated, or the answer itself when the store answered with
/// anything but a success. The body is read up to the broker's answer bound
/// ([`ViewOps::read_answer`], T-3773).
async fn read(view: &ViewOps, answer: Response) -> Result<Result<Upstream, Response>, Refusal> {
    if !answer.status().is_success() {
        return Ok(Err(answer));
    }
    let truncated = answer
        .headers()
        .get_all("NGSILD-Warning")
        .iter()
        .any(|w| w.as_bytes().windows(9).any(|at| at == b"truncated"));
    let total = answer
        .headers()
        .get("NGSILD-Results-Count")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let bytes = view.read_answer(answer.into_body(), "scan").await?;
    match serde_json::from_slice(&bytes).ok() {
        Some(Value::Array(entities)) => Ok(Ok((entities, total, truncated))),
        _ => Err(Refusal::Malformed(
            "the store's answer is no list of Entities",
        )),
    }
}

/// One upstream page: its Entities, its count and whether its join walk was truncated.
type Upstream = (Vec<Value>, Option<usize>, bool);

/// The members of an Attribute that hold its value, never an Attribute of their own.
const VALUES: [&str; 7] = [
    "value",
    "object",
    "json",
    "languageMap",
    "valueList",
    "objectList",
    "vocab",
];

/// Removes what only the residual asked for: the Attributes it added to `pick`, and
/// `createdAt`/`modifiedAt` of the Entity, its Attributes and their sub-Attributes when the
/// standard did not ask for them (a value's own members stay; the scan reads
/// `application/json`, so no `@context` member is there to keep). With `local`, nothing was
/// projected upstream and every Attribute it does not name goes.
fn strip(entity: &mut Value, extra: &[&str], system: bool, local: Option<&[String]>) {
    // a Query Entities answer holds objects only
    entity.as_object_mut().into_iter().for_each(|members| {
        for name in extra {
            members.remove(*name);
        }
        if let Some(keep) = local {
            members.retain(|k, _| antares_model::is_meta(k) || keep.iter().any(|s| s == k));
        }
        if system {
            unsystem(members);
        }
    });
}

fn unsystem(members: &mut serde_json::Map<String, Value>) {
    members.remove("createdAt");
    members.remove("modifiedAt");
    for (name, value) in members.iter_mut() {
        if VALUES.contains(&name.as_str()) {
            continue;
        }
        match value {
            Value::Object(m) => unsystem(m),
            Value::Array(items) => items
                .iter_mut()
                .filter_map(Value::as_object_mut)
                .for_each(unsystem),
            _ => {}
        }
    }
}
