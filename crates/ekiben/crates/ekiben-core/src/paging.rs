//! Paged pipeline execution: a stream's top-level processors run depth first, and a batch larger
//! than the stream's page is cut into pages wherever the rest of the pipeline treats each message
//! on its own, so a run holds one page's messages at a time instead of its whole source.
//!
//! An ekiben deviation from Bento, which runs `ExecuteAll` breadth first over one batch per input
//! message: the messages written and their order are the same, but the steps of different pages
//! interleave (page 1 is written before page 2 is read), batch-wide steps after a cut see one page
//! (none is allowed between a cut and the next `split`, see [`page_cuts`]), and a run that fails
//! part way has written the pages before the failure.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;

use serde_json::Value as JsonValue;

use crate::config::ProcessorConfig;
use crate::error::CoreError;
use crate::message::Batch;
use crate::traits::{Pages, ProcContext, Processor};

/// Messages one page holds when the pipeline names no `page_rows`.
pub const DEFAULT_PAGE_ROWS: usize = 1000;

/// Where a stream's batches may be cut into pages, and how large a page is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paging {
    /// Messages one page holds at most.
    pub rows: usize,
    /// `cuts[i]`: the batches processor `i` returns may be cut into pages.
    pub cuts: Vec<bool>,
}

impl Paging {
    /// The paging of a pipeline of `procs` with pages of `rows` messages.
    pub fn new(procs: &[ProcessorConfig], rows: usize) -> Self {
        Self {
            rows: rows.max(1),
            cuts: page_cuts(procs),
        }
    }

    fn cut_after(&self, at: usize) -> bool {
        self.cuts.get(at).copied().unwrap_or(false)
    }
}

/// Functions whose answer depends on the other messages of the batch: a step naming one sees a
/// different batch on each side of a cut.
const BATCH_WIDE: [&str; 4] = ["batch_index", "batch_size", "from_all", "from("];

/// Processors that read and write each message on its own (Bento runs them per message), so a
/// batch and its pages give the same messages. Each is still refused when its config names a
/// [`BATCH_WIDE`] function, in a mapping or an interpolation.
const PER_MESSAGE: [&str; 20] = [
    "bloblang",
    "bounds_check",
    "cache",
    "compress",
    "decompress",
    "dedupe",
    "grok",
    "jmespath",
    "jq",
    "json_schema",
    "log",
    "mapping",
    "metric",
    "mutation",
    "noop",
    "parquet_decode",
    "protobuf",
    "rate_limit",
    "sleep",
    "unarchive",
];

/// For each processor, whether the batches it returns may be cut into pages: every later step up
/// to the next `split` (which regroups the messages anyway) or the end of the pipeline reads each
/// message on its own. A step that joins the batch (`archive`, `group_by`, a `while` judged by the
/// first message, an unknown processor) after the cut keeps the batch whole.
pub fn page_cuts(procs: &[ProcessorConfig]) -> Vec<bool> {
    let mut cuts = vec![false; procs.len()];
    // Walking back: `safe` says whether everything after `at` up to a split or the end is safe.
    let mut safe = true;
    for at in (0..procs.len()).rev() {
        cuts[at] = safe;
        let p = &procs[at];
        safe = if p.component_type == "split" {
            true
        } else {
            safe && page_safe(&p.component_type, &p.config) && p.processors.is_empty()
        };
    }
    cuts
}

/// Whether one processor of kind `kind` gives the same messages over a batch and over its pages.
fn page_safe(kind: &str, conf: &JsonValue) -> bool {
    match kind {
        // Each message runs the children alone (`internal/impl/pure/processor_try.go` etc.).
        "try" | "catch" | "for_each" => true,
        "http" => {
            conf.get("batch_as_multipart").and_then(JsonValue::as_bool) != Some(true)
                && !names_batch_wide(conf)
        }
        "branch" => {
            let maps = ["request_map", "result_map"]
                .iter()
                .filter_map(|k| conf.get(*k))
                .any(names_batch_wide);
            !maps && children_safe(conf.get("processors"))
        }
        "switch" => conf.as_array().is_some_and(|cases| {
            cases.iter().all(|case| {
                !case.get("check").is_some_and(names_batch_wide)
                    && children_safe(case.get("processors"))
            })
        }),
        "processors" => children_safe(Some(conf)),
        _ => PER_MESSAGE.contains(&kind) && !names_batch_wide(conf),
    }
}

/// Whether every processor of a child list (raw config objects) is page safe; a `split` inside
/// regroups the child batch, which the parent then returns as one batch: not safe.
fn children_safe(children: Option<&JsonValue>) -> bool {
    let Some(children) = children else {
        return true;
    };
    let Some(list) = children.as_array() else {
        return false;
    };
    list.iter().all(|child| {
        let Some(obj) = child.as_object() else {
            return false;
        };
        let mut kinds = obj.iter().filter(|(k, _)| k.as_str() != "label");
        match (kinds.next(), kinds.next()) {
            (Some((kind, conf)), None) => kind != "split" && page_safe(kind, conf),
            _ => false,
        }
    })
}

fn names_batch_wide(conf: &JsonValue) -> bool {
    let text = conf.to_string();
    BATCH_WIDE.iter().any(|f| text.contains(f))
}

/// The batches waiting at one depth of the walk.
enum Level {
    Batches(VecDeque<Batch>),
    Pages(Box<Pages>),
}

impl Level {
    fn next(&mut self) -> Option<Batch> {
        match self {
            Level::Batches(batches) => batches.pop_front(),
            Level::Pages(pages) => pages.next(),
        }
    }

    /// `batches` with each one larger than `rows` cut into pages of `rows`.
    fn paged(batches: Vec<Batch>, rows: usize) -> Self {
        let mut out = VecDeque::with_capacity(batches.len());
        for mut batch in batches {
            while batch.len() > rows {
                let rest = batch.split_off(rows);
                out.push_back(batch);
                batch = rest;
            }
            out.push_back(batch);
        }
        Level::Batches(out)
    }
}

/// Runs `procs` over `batch` depth first and hands each batch that leaves the last processor to
/// `emit` as soon as it is done, in the order breadth-first execution
/// ([`crate::execute_all`]) would return them. Where `paging` allows, a batch of more than
/// `paging.rows` messages goes on as pages; a processor that can produce its result page by page
/// ([`Processor::pages`]) does, so the messages of later pages do not exist yet.
pub async fn execute_paged<E, F>(
    ctx: &ProcContext,
    procs: &[Arc<dyn Processor>],
    batch: Batch,
    paging: &Paging,
    mut emit: E,
) -> Result<(), CoreError>
where
    E: FnMut(Batch) -> F,
    F: Future<Output = Result<(), CoreError>>,
{
    let mut stack = vec![(0usize, Level::Batches(VecDeque::from([batch])))];
    while let Some((at, level)) = stack.last_mut() {
        let at = *at;
        let Some(batch) = level.next() else {
            stack.pop();
            continue;
        };
        let Some(proc) = procs.get(at) else {
            emit(batch).await?;
            continue;
        };
        if ctx.is_shutting_down() {
            return Err(CoreError::ComponentClosed);
        }
        let next = if paging.cut_after(at) {
            match proc.pages(ctx, batch, paging.rows) {
                Ok(pages) => Level::Pages(pages),
                Err(batch) => Level::paged(proc.process_batch(ctx, batch).await?, paging.rows),
            }
        } else {
            Level::Batches(proc.process_batch(ctx, batch).await?.into())
        };
        stack.push((at + 1, next));
    }
    Ok(())
}
