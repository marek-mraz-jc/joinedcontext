// SPDX-License-Identifier: EUPL-1.2
//! Files (T-4465, T-4477, EP-119, EP-08, EP-41). `items` as one file: `f=parquet` (Apache
//! Parquet, `GeoParquet` 1.1 with a geometry) or `f=csv`, or an `Accept` naming one,
//! holds the whole answer of the request's own filters (bbox, datetime, CQL2 `filter`, the
//! `ngsild.` options). `/collections` as one file: `f=zip` (a CSV per collection, and its
//! Parquet in a `parquet` build; no Excel, owner 2026-10-18), filtered by `bbox` and
//! `datetime`, narrowed by `collections`. A file holds one type per table; a CSV or a Parquet of
//! several collections is refused, naming the two ways to get them. The pages are read as
//! `items` reads them, so the view narrows a file exactly as it narrows the `GeoJSON`, and each
//! page is sent as it is read (jc-ogc-stream): a file holds one page in memory, never the
//! whole file, under its own byte and time caps.

use axum::http::HeaderMap;

/// The media type of a Parquet file.
#[cfg(feature = "parquet")]
pub(crate) const PARQUET: &str = "application/vnd.apache.parquet";

/// The files `items` writes in this build: the `f` value, the media type (its essence is what
/// an `Accept` names), and the writer.
#[cfg(any(feature = "parquet", feature = "tabular"))]
pub(crate) const ITEMS: &[(&str, &str, file::Make)] = &[
    #[cfg(feature = "parquet")]
    ("parquet", PARQUET, file::parquet),
    #[cfg(feature = "tabular")]
    ("csv", antares_export::csv::MEDIA_TYPE, file::csv),
];
/// Without a file feature `items` writes no file: an `f=` stays among the pairs and is refused
/// like any other unknown parameter.
#[cfg(not(any(feature = "parquet", feature = "tabular")))]
pub(crate) const ITEMS: &[(&str, &str, ())] = &[];

/// The files `/collections` writes of every collection the view shows.
#[cfg(feature = "tabular")]
pub(crate) const BUNDLES: &[(&str, &str)] = &[("zip", antares_export::bundle::MEDIA_TYPE)];
#[cfg(not(feature = "tabular"))]
pub(crate) const BUNDLES: &[(&str, &str)] = &[];

/// The `f` values of a file of one collection, refused on `/collections`.
#[cfg(feature = "tabular")]
pub(crate) const ONE_TYPE: &[&str] = &["csv", "parquet"];
#[cfg(not(feature = "tabular"))]
pub(crate) const ONE_TYPE: &[&str] = &[];

/// Whether the request asks for one of `offered` (`f` value, media type) by `f` or by its
/// `Accept`, and its pairs without the `f` that asked.
pub(crate) fn asked(
    headers: &HeaderMap,
    pairs: Vec<(String, String)>,
    offered: &[(&'static str, &'static str)],
) -> crate::Asked {
    let by_f = offered
        .iter()
        .find(|(f, _)| pairs.iter().any(|(n, v)| n == "f" && v == f));
    let pairs = pairs
        .into_iter()
        .filter(|(n, v)| !(n == "f" && by_f.is_some_and(|(f, _)| v == f)))
        .collect();
    let ranges: Vec<String> = headers
        .get_all(axum::http::header::ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|range| essence(range).to_owned())
        .collect();
    let by_accept = offered.iter().find(|(_, media)| {
        ranges
            .iter()
            .any(|r| r.eq_ignore_ascii_case(essence(media)))
    });
    crate::Asked {
        pairs,
        file: by_f.or(by_accept).map(|(f, _)| *f),
    }
}

/// How many files a router writes at once: `raw` when it is 1 to 64, else
/// [`crate::EXPORT_FILES`] (T-4465 review).
pub(crate) fn files_of(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.parse().ok())
        .filter(|n| (1..=64).contains(n))
        .unwrap_or(crate::EXPORT_FILES)
}

/// The bytes one file may hold unless `ANTARES_EXPORT_FILE_BYTES` names another (jc-ogc-stream):
/// the largest collection of the frozen prod export (praha-mesto Garden, 255,283 rows, at most
/// 192 MiB of CSV) as a ZIP counting its CSV and its Parquet unpacked (about 290 MiB), times 1.5,
/// rounded up to 512 MiB. A file is sent as it is written, so this bounds the download, not
/// memory; the answer ceiling (`MAX_ANSWER_BYTES`) stays the bound of every other answer.
pub(crate) const FILE_BYTES: usize = 512 << 20;

/// The seconds one file may take to write unless `ANTARES_EXPORT_FILE_SECONDS` names another:
/// that ZIP reads Garden's 256 pages twice (CSV, Parquet) at the 0.31 s a page a loaded restore
/// copy measured (jc-garden-zip; jc-ogc-keyset had assumed 88 ms), 159 s, times 1.5. At 120 s it
/// was cut at 119.9 s in its Parquet member. A slow reader holds its place as long, so six places
/// can refuse other files for this long.
pub(crate) const FILE_SECONDS: u64 = 240;

/// The rows one file may read unless `ANTARES_EXPORT_FILE_ROWS` names another (T-6823): a ZIP
/// reads a collection once per member (CSV, Parquet), so its rows count once per member. The jc
/// instance (the only host of this copy) wrote 377,000 rows of jc-dev's 421,438-row Garden CSV
/// in [`FILE_SECONDS`] (0.64 s a page of 1,000) and cut the rest; 300,000 rows take 192 s at
/// that rate. A file the store counts past it is `413` before its first byte instead of a
/// download cut at its time; a host with a faster store raises it with its time.
pub(crate) const FILE_ROWS: u64 = 300_000;

/// The rows a file may read: `raw` when it is 1 to 100,000,000, else [`FILE_ROWS`].
pub(crate) fn rows_of(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.parse().ok())
        .filter(|n| (1..=100_000_000).contains(n))
        .unwrap_or(FILE_ROWS)
}

/// The bytes a file may hold: `raw` when it is 1 MiB to 4 GiB less one byte (a ZIP member
/// past 4 GiB needs ZIP64), else [`FILE_BYTES`].
pub(crate) fn bytes_of(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.parse().ok())
        .filter(|n: &u64| (1 << 20..1 << 32).contains(n))
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(FILE_BYTES)
}

/// The seconds a file may take: `raw` when it is 1 to 3600, else [`FILE_SECONDS`].
pub(crate) fn seconds_of(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.parse().ok())
        .filter(|n| (1..=3600).contains(n))
        .unwrap_or(FILE_SECONDS)
}

/// A media type or range without its parameters.
fn essence(media: &str) -> &str {
    media.split(';').next().unwrap_or_default().trim()
}

#[cfg(any(feature = "parquet", feature = "tabular"))]
pub(crate) use file::{Budget, Gate};

#[cfg(any(feature = "parquet", feature = "tabular"))]
mod file {
    use super::ITEMS;
    use crate::model::Collection;
    use crate::{items, Here, Problem, Reply};
    use antares_export::{Column, Error, Kind, Sink};
    use antares_ops::{Caller, OperationId, ViewOps};
    use axum::body::{Body, Bytes};
    use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, RETRY_AFTER};
    use axum::http::{HeaderValue, StatusCode};
    use axum::response::IntoResponse as _;
    use futures_util::future::BoxFuture;
    use futures_util::StreamExt as _;
    use serde_json::Value;
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tracing::Instrument as _;

    /// Features an export reads per `items` page: the broker's default `max_limit`, so an
    /// export holds one store page at a time.
    const EXPORT_PAGE: u64 = 1000;

    /// The refusal past the cap of files written at once: `429` with `Retry-After`, so a client
    /// (and the gateway, which passes a `4xx` on) reads it as "ask again", never as a broker
    /// failure (T-6823).
    pub(crate) const BUSY: Problem = Problem(
        StatusCode::TOO_MANY_REQUESTS,
        "the broker is writing as many files as it writes at once: try again later",
    );

    /// The refusal of a file still being written when its time ran out.
    pub(crate) const TOO_SLOW: Problem = Problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "the file took longer than a file may take to write: narrow it (bbox, datetime, filter)",
    );

    /// The refusal of a file whose rows the store counts past [`Gate::rows`].
    pub(crate) const TOO_MANY_ROWS: Problem = Problem(
        StatusCode::PAYLOAD_TOO_LARGE,
        "the file would hold more rows than a file may: narrow it (bbox, datetime, filter)",
    );

    /// Why a file ended before its last byte when no other reason reached its body.
    const CUT: &str = "the file stopped before its end";

    /// The files one router writes at once (T-4465 review, `ANTARES_EXPORT_FILES`), and what one
    /// file may hold and take: its bytes (`ANTARES_EXPORT_FILE_BYTES`) and its time
    /// (`ANTARES_EXPORT_FILE_SECONDS`). A file is sent as it is written, so it holds one page in
    /// memory; a request past the cap is refused, never queued.
    #[derive(Debug, Clone)]
    pub(crate) struct Gate {
        writing: Arc<AtomicUsize>,
        cap: usize,
        pub(crate) bytes: usize,
        pub(crate) time: Duration,
        /// The rows one file may read (`ANTARES_EXPORT_FILE_ROWS`).
        pub(crate) rows: u64,
    }

    /// A file being written; dropping it frees its place.
    pub(crate) struct Slot(Arc<AtomicUsize>);

    impl Drop for Slot {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// What a file this surface writes is held to: its bytes and the gate's cap and time.
    pub(crate) struct Budget {
        pub(crate) max: usize,
        pub(crate) gate: Gate,
    }

    impl Budget {
        /// The gate the router put on the request.
        pub(crate) fn of(parts: &axum::http::request::Parts) -> Self {
            let gate: Gate = parts.extensions.get().cloned().unwrap_or_default();
            Self {
                max: gate.bytes,
                gate,
            }
        }
    }

    impl Gate {
        /// `cap` files at once, each held to the default bytes and time.
        #[cfg(test)]
        pub(crate) fn new(cap: usize) -> Self {
            Self::with_limits(
                cap,
                super::FILE_BYTES,
                Duration::from_secs(super::FILE_SECONDS),
            )
        }

        /// `cap` files at once, each of at most `bytes` written within `time`.
        pub(crate) fn with_limits(cap: usize, bytes: usize, time: Duration) -> Self {
            Self {
                writing: Arc::new(AtomicUsize::new(0)),
                cap,
                bytes,
                time,
                rows: super::FILE_ROWS,
            }
        }

        /// A place for one more file, or [`BUSY`] when `cap` are being written.
        pub(crate) fn take(&self) -> Result<Slot, Problem> {
            self.writing
                .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    n.checked_add(1).filter(|more| *more <= self.cap)
                })
                .map(|_| Slot(Arc::clone(&self.writing)))
                .map_err(|_| BUSY)
        }
    }

    /// The caps the deployment set.
    impl Default for Gate {
        fn default() -> Self {
            let var = |name| std::env::var(name).ok();
            Self {
                rows: super::rows_of(var(crate::EXPORT_FILE_ROWS_ENV).as_deref()),
                ..Self::with_limits(
                    super::files_of(var(crate::EXPORT_FILES_ENV).as_deref()),
                    super::bytes_of(var(crate::EXPORT_FILE_BYTES_ENV).as_deref()),
                    Duration::from_secs(super::seconds_of(
                        var(crate::EXPORT_FILE_SECONDS_ENV).as_deref(),
                    )),
                )
            }
        }
    }

    /// The refusal of a file before its first byte; the gate's `429` says when to ask again.
    pub(crate) fn refusal(refused: Problem) -> Reply {
        let mut reply = refused.reply();
        if refused == BUSY {
            reply
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static("6"));
        }
        reply
    }

    /// A writer of one collection's file: its name (a sheet's), its columns, whether it has a
    /// geometry, and the ceiling.
    pub(crate) type Make = fn(&str, Vec<Column>, bool, usize) -> Result<Box<dyn Sink>, Error>;

    /// The Parquet writer.
    #[cfg(feature = "parquet")]
    pub(crate) fn parquet(
        _: &str,
        columns: Vec<Column>,
        geometry: bool,
        max: usize,
    ) -> Result<Box<dyn Sink>, Error> {
        Ok(Box::new(antares_export::parquet::Writer::new(
            columns, geometry, max,
        )?))
    }

    /// The CSV writer.
    #[cfg(feature = "tabular")]
    pub(crate) fn csv(
        _: &str,
        columns: Vec<Column>,
        geometry: bool,
        max: usize,
    ) -> Result<Box<dyn Sink>, Error> {
        Ok(Box::new(antares_export::csv::Writer::new(
            columns, geometry, max,
        )?))
    }

    /// A file past its ceiling is refused before its first byte, and cut after it.
    impl From<Error> for Problem {
        fn from(e: Error) -> Self {
            match e {
                Error::TooLarge => Problem(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "the file is larger than a file may be: narrow it (bbox, datetime, filter)",
                ),
                Error::Write(_) => Problem(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "the file could not be written",
                ),
            }
        }
    }

    /// A column per property the view shows (those `properties` keeps), typed from the
    /// collection's schema.
    pub(crate) fn columns(c: &Collection, p: &items::Params) -> Vec<Column> {
        c.properties
            .iter()
            .filter(|prop| {
                p.properties
                    .as_ref()
                    .is_none_or(|keep| keep.contains(&prop.iri))
            })
            .map(|prop| Column {
                name: prop.name.clone(),
                kind: Kind::of_schema(&prop.schema),
            })
            .collect()
    }

    /// Where a file's pages come from, one at a time.
    pub(crate) trait Pages: Send {
        /// The next page of Features, `None` after the last.
        fn next(&mut self) -> BoxFuture<'_, Result<Option<Vec<Value>>, Problem>>;

        /// The Features every page holds together as the store counts them before the first
        /// page, `None` when it cannot say.
        fn matched(&mut self) -> BoxFuture<'_, Result<Option<u64>, Problem>> {
            Box::pin(async { Ok(None) })
        }
    }

    /// [`TOO_MANY_ROWS`] when the rows `pages` read together pass `rows`; a file whose count
    /// the store cannot state is written and held to its bytes and time.
    pub(crate) async fn within_rows(
        pages: &mut [&mut dyn Pages],
        rows: u64,
    ) -> Result<(), Problem> {
        let mut read = 0u64;
        for p in pages.iter_mut() {
            match p.matched().await? {
                Some(n) => read = read.saturating_add(n),
                None => return Ok(()),
            }
        }
        if read > rows {
            return Err(TOO_MANY_ROWS);
        }
        Ok(())
    }

    /// The pages of `items` of one collection, read as `items` reads them: each page is a Query
    /// Entities through [`ViewOps::run`], so the view, its Tenant and the Policy narrow every
    /// page, however long after its handler the body reads it.
    pub(crate) struct Read {
        view: ViewOps,
        caller: Caller,
        c: Collection,
        p: items::Params,
        digest: u64,
        base: String,
        /// The in-process call depth the request's handler ran at.
        depth: usize,
        done: bool,
    }

    impl Pages for Read {
        fn next(&mut self) -> BoxFuture<'_, Result<Option<Vec<Value>>, Problem>> {
            Box::pin(async move {
                if self.done {
                    return Ok(None);
                }
                let refs = items::Refs {
                    profile: self.p.profile,
                    base: &self.base,
                    alternate: self.view.allows(OperationId::RetrieveEntity),
                };
                let read = crate::features::page(
                    &self.view,
                    &self.caller,
                    &self.c,
                    &self.p,
                    self.digest,
                    refs,
                );
                // The body reads outside the in-process call that answered it (ADR-0025 rule
                // 3): each page goes at the depth its handler ran at, never from zero.
                let page = Box::pin(antares_ops::at_call_depth(self.depth, read)).await?;
                // A token that does not move would read the same page again.
                match page.next.filter(|next| self.p.next.as_ref() != Some(next)) {
                    Some(next) => self.p.next = Some(next),
                    None => self.done = true,
                }
                Ok(Some(page.features))
            })
        }

        /// One Feature read with its count, before the first page (T-6823).
        fn matched(&mut self) -> BoxFuture<'_, Result<Option<u64>, Problem>> {
            Box::pin(async move {
                let (limit, uncounted) = (self.p.limit, self.p.uncounted);
                (self.p.limit, self.p.uncounted) = (1, false);
                let refs = items::Refs {
                    profile: self.p.profile,
                    base: &self.base,
                    alternate: false,
                };
                let read = crate::features::page(
                    &self.view,
                    &self.caller,
                    &self.c,
                    &self.p,
                    self.digest,
                    refs,
                );
                let page = Box::pin(antares_ops::at_call_depth(self.depth, read)).await;
                (self.p.limit, self.p.uncounted) = (limit, uncounted);
                Ok(page?.matched)
            })
        }
    }

    /// What the task writing a file hands its body: the bytes to send next.
    pub(crate) type Chunk = Vec<u8>;

    /// The body of a file went away: the client left, or the body was dropped.
    const GONE: Problem = Problem(StatusCode::INTERNAL_SERVER_ERROR, CUT);

    /// What a file's task wrote so far, for the line a cut file logs.
    #[derive(Default)]
    pub(crate) struct Tally {
        rows: AtomicUsize,
        bytes: AtomicUsize,
    }

    tokio::task_local! {
        static TALLY: Arc<Tally>;
    }

    /// `n` more rows written into the file of the task this runs on.
    pub(crate) fn tally_rows(n: usize) {
        let _ = TALLY.try_with(|t| t.rows.fetch_add(n, Ordering::Relaxed));
    }

    /// Sends `bytes` on, unless there are none.
    pub(crate) async fn send(tx: &mpsc::Sender<Chunk>, bytes: Vec<u8>) -> Result<(), Problem> {
        if bytes.is_empty() {
            return Ok(());
        }
        let n = bytes.len();
        tx.send(bytes).await.map_err(|_| GONE)?;
        let _ = TALLY.try_with(|t| t.bytes.fetch_add(n, Ordering::Relaxed));
        Ok(())
    }

    /// A file's place and the task writing it, held by its body: dropping the body (its end, or
    /// a client that goes away) frees the place at once and stops the task.
    struct Writing {
        task: tokio::task::AbortHandle,
        _slot: Slot,
    }

    impl Drop for Writing {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    /// How a file of one media type is sent.
    pub(crate) struct Sent<'a> {
        pub(crate) media: &'a str,
        pub(crate) name: &'a str,
        /// The record a CSV ends with when it stops early, `None` for a format that has none.
        pub(crate) trailer: Option<fn(&str) -> String>,
    }

    /// The last record of a CSV cut short: why. The transfer still ends broken, so no client
    /// reads the rows before it as the whole file.
    pub(crate) fn csv_trailer(detail: &str) -> String {
        format!("# {CUT}: {detail}\r\n")
    }

    /// The file `work` writes into its channel, sent as it is written. `work` runs on its own
    /// task within `time`, and the body holds `slot`: the first chunk (or the refusal before it) sets the
    /// status, so a file refused before its first byte keeps its status. After it, a refusal,
    /// the time running out or a task that ends without finishing the file ends the body with
    /// an error (and the CSV trailer), never as a whole file, and the task logs it once at `WARN`
    /// under the request's span. Dropping the body stops the task and frees `slot`.
    pub(crate) async fn streamed<W, F>(
        sent: &Sent<'_>,
        slot: Slot,
        time: Duration,
        work: W,
    ) -> Reply
    where
        W: FnOnce(mpsc::Sender<Chunk>) -> F,
        F: Future<Output = Result<(), Problem>> + Send + 'static,
    {
        let (tx, mut rx) = mpsc::channel::<Chunk>(1);
        // How the file ended, set before the task's last sender drops: a body that sees the
        // channel close reads why there.
        let outcome = Arc::new(OnceLock::<Result<(), Problem>>::new());
        let open = tx.clone();
        let written = work(tx);
        let ended = Arc::clone(&outcome);
        let file = sent.name.to_owned();
        let task = tokio::spawn(
            async move {
                let started = tokio::time::Instant::now();
                let tally = Arc::new(Tally::default());
                let result = TALLY
                    .scope(Arc::clone(&tally), tokio::time::timeout(time, written))
                    .await
                    .unwrap_or(Err(TOO_SLOW));
                if let Err(Problem(status, reason)) = &result {
                    tracing::warn!(
                        file,
                        reason,
                        status = status.as_u16(),
                        rows = tally.rows.load(Ordering::Relaxed),
                        bytes = tally.bytes.load(Ordering::Relaxed),
                        seconds = started.elapsed().as_secs_f64(),
                        "an OGC file stopped before its end"
                    );
                }
                let _ = ended.set(result);
                drop(open);
            }
            .instrument(tracing::Span::current()),
        );
        let guard = Writing {
            task: task.abort_handle(),
            _slot: slot,
        };
        let first = match rx.recv().await {
            Some(first) => first,
            None => match outcome.get() {
                Some(Ok(())) => Vec::new(),
                Some(Err(refused)) => return refusal(*refused),
                None => return refusal(Problem::from(Error::Write(String::new()))),
            },
        };
        let trailer = sent.trailer;
        let rest = futures_util::stream::unfold(Some((rx, guard)), move |state| {
            let outcome = Arc::clone(&outcome);
            async move {
                let (mut rx, guard) = state?;
                if let Some(bytes) = rx.recv().await {
                    return Some((vec![Ok(Bytes::from(bytes))], Some((rx, guard))));
                }
                let detail: &'static str = match outcome.get() {
                    Some(Ok(())) => return None,
                    Some(Err(refused)) => refused.1,
                    None => CUT,
                };
                let mut last: Vec<Result<Bytes, std::io::Error>> = Vec::new();
                if let Some(trailer) = trailer {
                    last.push(Ok(Bytes::from(trailer(detail))));
                }
                last.push(Err(std::io::Error::other(detail)));
                Some((last, None))
            }
        })
        .flat_map(futures_util::stream::iter);
        let body = futures_util::stream::iter([Ok(Bytes::from(first))]).chain(rest);
        (
            StatusCode::OK,
            [
                (CONTENT_TYPE, sent.media.to_owned()),
                (
                    CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", sent.name),
                ),
            ],
            Body::from_stream(body),
        )
            .into_response()
    }

    /// Every page of `pages` written into `sink`, its bytes sent after each page; the number of
    /// Features.
    pub(crate) async fn pages_into(
        tx: &mpsc::Sender<Chunk>,
        pages: &mut dyn Pages,
        sink: &mut dyn Sink,
    ) -> Result<usize, Problem> {
        let mut rows = 0usize;
        while let Some(page) = pages.next().await? {
            sink.page(&page)?;
            rows = rows.saturating_add(page.len());
            tally_rows(page.len());
            send(tx, sink.drain()?).await?;
        }
        Ok(rows)
    }

    /// One file: every page of `pages` through `sink`, then its end with `provenance` and the
    /// number of rows.
    pub(crate) async fn one_file(
        tx: mpsc::Sender<Chunk>,
        mut pages: Box<dyn Pages>,
        mut sink: Box<dyn Sink>,
        provenance: Vec<(String, String)>,
    ) -> Result<(), Problem> {
        let rows = pages_into(&tx, pages.as_mut(), sink.as_mut()).await?;
        let end = sink.finish(&with_rows(provenance, rows))?;
        send(&tx, end).await
    }

    /// `provenance` naming `rows`.
    pub(crate) fn with_rows(
        mut provenance: Vec<(String, String)>,
        rows: usize,
    ) -> Vec<(String, String)> {
        for (key, value) in &mut provenance {
            if key == "rows" {
                *value = rows.to_string();
            }
        }
        provenance
    }

    impl Here<'_> {
        /// `items` of `c` under `pairs` as the file `f` names.
        pub(crate) async fn file(
            &self,
            c: &Collection,
            pairs: &[(String, String)],
            f: &str,
        ) -> Reply {
            let missing = Problem(StatusCode::NOT_ACCEPTABLE, "no such file");
            let Some((_, media, make)) = ITEMS.iter().find(|e| e.0 == f) else {
                return refusal(missing);
            };
            let name = format!("{}.{f}", c.id);
            let started = self.budget.gate.take().and_then(|slot| {
                let p = items::params(c, pairs)?;
                if p.crs != crate::crs::CRS84 {
                    return Err(Problem(
                        StatusCode::BAD_REQUEST,
                        "a file is written in CRS84: leave crs out or name CRS84",
                    ));
                }
                let sink = make(&c.id, columns(c, &p), c.geometry.is_some(), self.budget.max)?;
                let provenance = self.provenance(&[c.id.as_str()], pairs, 0);
                Ok((slot, self.read(c, p, pairs), sink, provenance))
            });
            let (slot, mut pages, sink, provenance) = match started {
                Ok(started) => started,
                Err(refused) => return refusal(refused),
            };
            if let Err(refused) = within_rows(&mut [&mut pages], self.budget.gate.rows).await {
                return refusal(refused);
            }
            let sent = Sent {
                media,
                name: &name,
                trailer: (*f == *"csv").then_some(csv_trailer as fn(&str) -> String),
            };
            streamed(&sent, slot, self.budget.gate.time, move |tx| {
                one_file(tx, Box::new(pages), sink, provenance)
            })
            .await
        }

        /// The pages of `items` of `c` under `p`, read by [`EXPORT_PAGE`] (or the broker's
        /// smaller page) without a count, so each page seeks past the one before by the store's
        /// keyset cursor (T-4907).
        pub(crate) fn read(
            &self,
            c: &Collection,
            mut p: items::Params,
            pairs: &[(String, String)],
        ) -> Read {
            let page = u64::try_from(self.view.page_size()).unwrap_or(EXPORT_PAGE);
            p.limit = EXPORT_PAGE.min(page).max(1);
            p.uncounted = true;
            Read {
                view: self.view.clone(),
                caller: self.caller.clone(),
                c: c.clone(),
                p,
                digest: items::digest(self.id, &c.id, pairs),
                base: self.base.to_owned(),
                depth: antares_ops::call_depth(),
                done: false,
            }
        }

        /// What produced a file: the view, the collections, the query, the rows, when, and
        /// which broker.
        pub(crate) fn provenance(
            &self,
            collections: &[&str],
            pairs: &[(String, String)],
            rows: usize,
        ) -> Vec<(String, String)> {
            let query: Vec<String> = pairs.iter().map(|(n, v)| format!("{n}={v}")).collect();
            let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            [
                ("view", self.id.to_owned()),
                ("collections", collections.join(",")),
                ("query", query.join("&")),
                ("rows", rows.to_string()),
                ("exportedAt", now),
                ("broker", format!("antares {}", env!("CARGO_PKG_VERSION"))),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect()
        }
    }
}

#[cfg(feature = "tabular")]
mod bundle {
    use super::file::{
        columns, refusal, send, streamed, tally_rows, with_rows, within_rows, Chunk, Pages, Sent,
    };
    use crate::model::Collection;
    use crate::{items, Here, Problem, Reply};
    use antares_export::bundle::Bundle;
    use antares_export::Sink;
    use axum::http::StatusCode;
    use serde_json::{Map, Value};
    use std::fmt::Write as _;
    use tokio::sync::mpsc;

    /// The filters a file of several collections takes, and those collections.
    type Chosen<'c> = (Vec<(String, String)>, Vec<&'c Collection>);

    /// One file of a bundle: its path, its writer, its pages and its provenance; `counted`
    /// when its rows count once in the bundle's (a collection's CSV, not its Parquet too).
    pub(crate) struct Member {
        pub(crate) path: String,
        pub(crate) extension: &'static str,
        pub(crate) sink: Box<dyn Sink>,
        pub(crate) pages: Box<dyn Pages>,
        pub(crate) provenance: Vec<(String, String)>,
        pub(crate) counted: bool,
    }

    /// Every member, each written page by page into `bundle` as its pages are read, then the
    /// `provenance.json` and `README.md` `close` writes for the bundle's rows and files.
    pub(crate) async fn members_into(
        tx: mpsc::Sender<Chunk>,
        mut bundle: Bundle,
        members: Vec<Member>,
        close: impl FnOnce(usize, &[(String, &str)]) -> Vec<(String, Vec<u8>)> + Send,
    ) -> Result<(), Problem> {
        let mut rows = 0usize;
        let mut files = Vec::new();
        for mut m in members {
            bundle.start(&m.path)?;
            let mut read = 0usize;
            while let Some(page) = m.pages.next().await? {
                m.sink.page(&page)?;
                read = read.saturating_add(page.len());
                tally_rows(page.len());
                bundle.write(&m.sink.drain()?)?;
                send(&tx, bundle.drain()).await?;
            }
            bundle.write(&m.sink.finish(&with_rows(m.provenance, read))?)?;
            send(&tx, bundle.drain()).await?;
            if m.counted {
                rows = rows.saturating_add(read);
            }
            files.push((m.path, m.extension));
        }
        for (path, bytes) in close(rows, &files) {
            bundle.add(&path, &bytes)?;
        }
        let end = bundle.finish()?;
        send(&tx, end).await
    }

    impl Here<'_> {
        /// Every collection the view shows (or those `collections` names) as one `f=zip`: a
        /// CSV each (and its Parquet in a `parquet` build), with a README and the provenance.
        /// The CSV and the Parquet of a collection read its pages each, so the bundle holds one
        /// page at a time, never a whole member.
        pub(crate) async fn bundle(&self, pairs: &[(String, String)], f: &str) -> Reply {
            let name = format!("{}.{f}", self.id);
            let started = self.chosen(pairs).and_then(|(filters, chosen)| {
                let slot = self.budget.gate.take()?;
                Ok((slot, self.members(&filters, &chosen, pairs)?, chosen))
            });
            let (slot, mut members, chosen) = match started {
                Ok(started) => started,
                Err(refused) => return refusal(refused),
            };
            let mut reads: Vec<&mut dyn Pages> =
                members.iter_mut().map(|m| m.pages.as_mut() as _).collect();
            if let Err(refused) = within_rows(&mut reads, self.budget.gate.rows).await {
                return refusal(refused);
            }
            let ids: Vec<&str> = chosen.iter().map(|c| c.id.as_str()).collect();
            let provenance = self.provenance(&ids, pairs, 0);
            let view = self.id.to_owned();
            let close = move |rows: usize, files: &[(String, &str)]| {
                let provenance = with_rows(provenance, rows);
                let record: Map<String, Value> = provenance
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::from(v.as_str())))
                    .collect();
                vec![
                    (
                        "provenance.json".to_owned(),
                        Value::Object(record).to_string().into_bytes(),
                    ),
                    (
                        "README.md".to_owned(),
                        readme(&view, &provenance, files).into_bytes(),
                    ),
                ]
            };
            let sent = Sent {
                media: antares_export::bundle::MEDIA_TYPE,
                name: &name,
                trailer: None,
            };
            let bundle = Bundle::new(self.budget.max);
            streamed(&sent, slot, self.budget.gate.time, move |tx| {
                members_into(tx, bundle, members, close)
            })
            .await
        }

        /// A CSV member per collection, and its Parquet in a `parquet` build, each with its own
        /// read of the collection's pages.
        fn members(
            &self,
            filters: &[(String, String)],
            chosen: &[&Collection],
            pairs: &[(String, String)],
        ) -> Result<Vec<Member>, Problem> {
            let mut members = Vec::new();
            for c in chosen {
                let (geometry, max) = (c.geometry.is_some(), self.budget.max);
                let writers: &[(&'static str, super::file::Make)] = &[
                    ("csv", super::file::csv),
                    #[cfg(feature = "parquet")]
                    ("parquet", super::file::parquet),
                ];
                for (extension, make) in writers {
                    let p = items::params(c, filters)?;
                    members.push(Member {
                        path: format!("{}.{extension}", c.id),
                        extension,
                        sink: make(&c.id, columns(c, &p), geometry, max)?,
                        pages: Box::new(self.read(c, p, filters)),
                        provenance: self.provenance(&[c.id.as_str()], pairs, 0),
                        counted: *extension == "csv",
                    });
                }
            }
            Ok(members)
        }

        /// The filters every collection takes (`bbox`, `datetime`) and the collections named,
        /// every current-state one when `collections` is absent: a `{type}-history` reads the
        /// type's whole history, so it joins a bundle only when named (T-5100).
        fn chosen(&self, pairs: &[(String, String)]) -> Result<Chosen<'_>, Problem> {
            let mut filters = Vec::new();
            let mut chosen: Vec<&Collection> =
                self.collections.values().filter(|c| !c.history).collect();
            for (name, value) in pairs {
                match name.as_str() {
                    "bbox" | "datetime" => filters.push((name.clone(), value.clone())),
                    "collections" => {
                        let unknown = Problem(
                            StatusCode::BAD_REQUEST,
                            "collections names collections of the view",
                        );
                        chosen = value
                            .split(',')
                            .map(|id| self.collections.get(id))
                            .collect::<Option<_>>()
                            .ok_or(unknown)?;
                    }
                    _ => {
                        return Err(Problem(
                            StatusCode::BAD_REQUEST,
                            "a file of several collections takes bbox, datetime and collections",
                        ))
                    }
                }
            }
            Ok((filters, chosen))
        }
    }

    /// What the bundle holds and where it came from, for a reader without the platform.
    fn readme(view: &str, provenance: &[(String, String)], files: &[(String, &str)]) -> String {
        let at = provenance
            .iter()
            .find(|(k, _)| k == "exportedAt")
            .map(|(_, v)| v.as_str())
            .unwrap_or_default();
        let mut text = format!(
            "# {view}\n\nThe collections of the view `{view}`, exported at {at}: one file per \
             collection, one row per Feature.\n\n| File | Holds |\n|---|---|\n"
        );
        for (file, extension) in files {
            let holds = if *extension == "csv" {
                "the collection as CSV"
            } else {
                "the collection as (Geo)Parquet"
            };
            let _ = writeln!(text, "| `{file}` | {holds} |");
        }
        text.push_str(
            "| `provenance.json` | the view, the collections, the query, the rows, when, and \
             which broker |\n",
        );
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect()
    }

    const OFFERED: &[(&str, &str)] = &[
        ("parquet", "application/vnd.apache.parquet"),
        ("csv", "text/csv; charset=utf-8; header=present"),
    ];

    #[test]
    fn a_file_is_asked_by_f_or_by_accept() {
        let none = HeaderMap::new();
        let asked_for = asked(&none, pairs(&[("f", "csv"), ("limit", "5")]), OFFERED);
        assert_eq!(asked_for.file, Some("csv"));
        assert_eq!(
            asked_for.pairs,
            pairs(&[("limit", "5")]),
            "the f is the encoding, no filter"
        );
        let mut accept = HeaderMap::new();
        accept.insert(
            axum::http::header::ACCEPT,
            HeaderValue::from_static("application/json;q=0.5, TEXT/CSV;q=0.9"),
        );
        assert_eq!(asked(&accept, Vec::new(), OFFERED).file, Some("csv"));
        accept.insert(axum::http::header::ACCEPT, HeaderValue::from_static("*/*"));
        assert_eq!(
            asked(&accept, Vec::new(), OFFERED).file,
            None,
            "anything is GeoJSON"
        );
        let asked_for = asked(&none, pairs(&[("f", "gpkg")]), OFFERED);
        assert_eq!(asked_for.file, None);
        assert_eq!(
            asked_for.pairs,
            pairs(&[("f", "gpkg")]),
            "another f stays to be refused"
        );
    }

    #[test]
    fn f_asks_for_a_file_only_in_a_build_that_writes_it() {
        let offered: Vec<(&str, &str)> = ITEMS.iter().map(|e| (e.0, e.1)).collect();
        let asked_for = asked(&HeaderMap::new(), pairs(&[("f", "parquet")]), &offered);
        assert_eq!(asked_for.file.is_some(), cfg!(feature = "parquet"));
        assert_eq!(asked_for.pairs.is_empty(), asked_for.file.is_some());
    }

    #[cfg(any(feature = "parquet", feature = "tabular"))]
    #[test]
    fn a_file_the_writer_refuses_is_a_500_naming_nothing() {
        let refused = crate::Problem::from(antares_export::Error::Write("x".into()));
        assert_eq!(
            refused,
            crate::Problem(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "the file could not be written"
            )
        );
    }

    #[test]
    fn a_file_holds_1_mib_to_4_gib_and_takes_1_to_3600_seconds() {
        assert_eq!(bytes_of(None), FILE_BYTES);
        assert_eq!(bytes_of(Some("1048576")), 1 << 20);
        assert_eq!(bytes_of(Some("4294967295")), (1 << 32) - 1);
        for wrong in ["1048575", "4294967296", "-1", "big", ""] {
            assert_eq!(bytes_of(Some(wrong)), FILE_BYTES, "{wrong}");
        }
        assert_eq!(seconds_of(None), FILE_SECONDS);
        assert_eq!(seconds_of(Some("1")), 1);
        assert_eq!(seconds_of(Some("3600")), 3600);
        for wrong in ["0", "3601", "-5", "soon", ""] {
            assert_eq!(seconds_of(Some(wrong)), FILE_SECONDS, "{wrong}");
        }
    }

    #[test]
    fn a_file_reads_1_to_100_million_rows() {
        assert_eq!(rows_of(None), FILE_ROWS);
        assert_eq!(rows_of(Some("1")), 1);
        assert_eq!(rows_of(Some("100000000")), 100_000_000);
        for wrong in ["0", "100000001", "-1", "many", ""] {
            assert_eq!(rows_of(Some(wrong)), FILE_ROWS, "{wrong}");
        }
    }

    /// Pages that state a count and read nothing.
    #[cfg(any(feature = "parquet", feature = "tabular"))]
    struct Counted(Option<u64>);

    #[cfg(any(feature = "parquet", feature = "tabular"))]
    impl file::Pages for Counted {
        fn next(
            &mut self,
        ) -> futures_util::future::BoxFuture<
            '_,
            Result<Option<Vec<serde_json::Value>>, crate::Problem>,
        > {
            Box::pin(async { Ok(None) })
        }

        fn matched(
            &mut self,
        ) -> futures_util::future::BoxFuture<'_, Result<Option<u64>, crate::Problem>> {
            let n = self.0;
            Box::pin(async move { Ok(n) })
        }
    }

    /// T-6823: the rows of every read of a file count together; one past the ceiling is `413`,
    /// a read the store cannot count lets the file be written.
    #[cfg(any(feature = "parquet", feature = "tabular"))]
    #[tokio::test]
    async fn the_rows_of_every_read_count_against_the_ceiling() {
        let (mut csv, mut parquet) = (Counted(Some(150)), Counted(Some(150)));
        assert_eq!(
            file::within_rows(&mut [&mut csv, &mut parquet], 300).await,
            Ok(())
        );
        let (mut csv, mut parquet) = (Counted(Some(150)), Counted(Some(151)));
        assert_eq!(
            file::within_rows(&mut [&mut csv, &mut parquet], 300).await,
            Err(file::TOO_MANY_ROWS)
        );
        assert_eq!(
            file::refusal(file::TOO_MANY_ROWS).status(),
            axum::http::StatusCode::PAYLOAD_TOO_LARGE
        );
        let (mut csv, mut unknown) = (Counted(Some(301)), Counted(None));
        assert_eq!(
            file::within_rows(&mut [&mut unknown, &mut csv], 300).await,
            Ok(())
        );
    }

    #[test]
    fn the_cap_of_files_is_one_to_64_else_two() {
        assert_eq!(files_of(None), crate::EXPORT_FILES);
        assert_eq!(files_of(Some("5")), 5);
        for wrong in ["0", "65", "-1", "two", ""] {
            assert_eq!(files_of(Some(wrong)), crate::EXPORT_FILES, "{wrong}");
        }
    }

    #[cfg(any(feature = "parquet", feature = "tabular"))]
    #[test]
    fn the_third_file_at_once_is_refused_with_retry_after() {
        let gate = file::Gate::new(2);
        let first = gate.take();
        let second = gate.take();
        assert!(first.is_ok(), "the first file");
        assert!(second.is_ok(), "the second file");
        let third = gate.take().err();
        assert_eq!(third, Some(file::BUSY));
        let reply = file::refusal(file::BUSY);
        assert_eq!(reply.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            reply.headers().get(axum::http::header::RETRY_AFTER),
            Some(&HeaderValue::from_static("6"))
        );
        drop(first);
        assert!(gate.take().is_ok(), "a written file frees its place");
        let refused = file::refusal(crate::Problem(axum::http::StatusCode::BAD_REQUEST, "x"));
        assert!(refused
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .is_none());
    }

    /// Pages handed out one at a time; after the first `free` each waits for a permit of
    /// `gate`, so a test holds the file's read where it chooses.
    #[cfg(any(feature = "parquet", feature = "tabular"))]
    struct Held {
        pages: std::collections::VecDeque<Vec<serde_json::Value>>,
        free: usize,
        gate: std::sync::Arc<tokio::sync::Semaphore>,
        read: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[cfg(any(feature = "parquet", feature = "tabular"))]
    impl file::Pages for Held {
        fn next(
            &mut self,
        ) -> futures_util::future::BoxFuture<
            '_,
            Result<Option<Vec<serde_json::Value>>, crate::Problem>,
        > {
            Box::pin(async move {
                if self.free == 0 {
                    if let Ok(permit) = self.gate.acquire().await {
                        permit.forget();
                    }
                } else {
                    self.free = self.free.saturating_sub(1);
                }
                let page = self.pages.pop_front();
                if page.is_some() {
                    self.read.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                Ok(page)
            })
        }
    }

    /// Three pages of one Car each, the first `free` of them handed out at once; the gate
    /// that releases the rest, and the count of pages read.
    #[cfg(any(feature = "parquet", feature = "tabular"))]
    fn held(
        free: usize,
    ) -> (
        Box<Held>,
        std::sync::Arc<tokio::sync::Semaphore>,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let read = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let pages = ["a", "b", "c"]
            .iter()
            .map(|n| vec![serde_json::json!({"id": format!("urn:{n}"), "properties": {}})])
            .collect();
        let pages = Box::new(Held {
            pages,
            free,
            gate: std::sync::Arc::clone(&gate),
            read: std::sync::Arc::clone(&read),
        });
        (pages, gate, read)
    }

    #[cfg(any(feature = "parquet", feature = "tabular"))]
    type Frame = Option<Result<Vec<u8>, String>>;

    /// The next frame of `body` within two seconds: its bytes, the error that ended it, or
    /// `None` at its end.
    #[cfg(any(feature = "parquet", feature = "tabular"))]
    async fn frame(body: &mut axum::body::Body) -> Result<Frame, String> {
        use http_body_util::BodyExt as _;
        let next = tokio::time::timeout(std::time::Duration::from_secs(2), body.frame()).await;
        Ok(match next.map_err(|_| "no frame within two seconds")? {
            None => None,
            Some(Ok(f)) => Some(Ok(f.into_data().map(|d| d.to_vec()).unwrap_or_default())),
            Some(Err(e)) => Some(Err(e.to_string())),
        })
    }

    /// Every frame left of `body`: the bytes, and the error that ended it.
    #[cfg(any(feature = "parquet", feature = "tabular"))]
    async fn rest(body: &mut axum::body::Body) -> Result<(Vec<u8>, Option<String>), String> {
        let mut bytes = Vec::new();
        loop {
            match frame(body).await? {
                None => return Ok((bytes, None)),
                Some(Ok(more)) => bytes.extend(more),
                Some(Err(e)) => return Ok((bytes, Some(e))),
            }
        }
    }

    #[cfg(feature = "tabular")]
    fn csv_sink(max: usize) -> Result<Box<dyn antares_export::Sink>, antares_export::Error> {
        file::csv("Car", Vec::new(), false, max)
    }

    #[cfg(feature = "tabular")]
    fn csv_sent() -> file::Sent<'static> {
        file::Sent {
            media: "text/csv",
            name: "Car.csv",
            trailer: Some(file::csv_trailer),
        }
    }

    /// The file `pages` make through `sink`, under `gate`'s bytes and `time`.
    #[cfg(feature = "tabular")]
    async fn csv_file(
        gate: &file::Gate,
        pages: Box<Held>,
        time: std::time::Duration,
    ) -> Result<axum::response::Response, String> {
        let slot = gate.take().map_err(|p| p.1.to_owned())?;
        let sink = csv_sink(gate.bytes).map_err(|e| e.to_string())?;
        Ok(file::streamed(&csv_sent(), slot, time, move |tx| {
            file::one_file(tx, pages, sink, Vec::new())
        })
        .await)
    }

    #[cfg(any(feature = "parquet", feature = "tabular"))]
    const SECOND: std::time::Duration = std::time::Duration::from_secs(1);

    /// The CSV's first rows leave while its next page is still being read: a buffered file
    /// would send nothing before its last page.
    #[cfg(feature = "tabular")]
    #[tokio::test]
    async fn a_csv_leaves_before_its_last_page_is_read() -> Result<(), String> {
        let gate = file::Gate::new(2);
        let (pages, release, read) = held(1);
        let reply = csv_file(&gate, pages, SECOND).await?;
        assert_eq!(reply.status(), axum::http::StatusCode::OK);
        let mut body = reply.into_body();
        let first = frame(&mut body).await?;
        assert_eq!(first, Some(Ok(b"id\r\nurn:a\r\n".to_vec())));
        assert_eq!(
            read.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "page 2 waits"
        );
        release.add_permits(10);
        let (more, error) = rest(&mut body).await?;
        assert_eq!(error, None);
        assert_eq!(more, b"urn:b\r\nurn:c\r\n");
        let both = (gate.take(), gate.take());
        assert!(
            both.0.is_ok() && both.1.is_ok(),
            "a file sent frees its place"
        );
        Ok(())
    }

    /// The Parquet's magic and first row group leave before its next page is read; the footer
    /// comes last and the whole file reads back.
    #[cfg(feature = "parquet")]
    #[tokio::test]
    async fn a_parquet_leaves_before_its_last_page_is_read() -> Result<(), String> {
        use parquet::file::reader::FileReader as _;
        let gate = file::Gate::new(2);
        let slot = gate.take().map_err(|p| p.1.to_owned())?;
        let (pages, release, read) = held(1);
        let sink = file::parquet("Car", Vec::new(), false, 1 << 20).map_err(|e| e.to_string())?;
        let sent = file::Sent {
            media: PARQUET,
            name: "Car.parquet",
            trailer: None,
        };
        let reply = file::streamed(&sent, slot, SECOND, move |tx| {
            file::one_file(tx, pages, sink, Vec::new())
        })
        .await;
        let mut body = reply.into_body();
        let Some(Ok(first)) = frame(&mut body).await? else {
            return Err("no first frame".into());
        };
        assert!(first.starts_with(b"PAR1"), "the magic and row group 1");
        assert_eq!(
            read.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "page 2 waits"
        );
        release.add_permits(10);
        let (more, error) = rest(&mut body).await?;
        assert_eq!(error, None);
        let file = [first, more].concat();
        assert!(file.ends_with(b"PAR1"));
        let file = parquet::file::reader::SerializedFileReader::new(bytes::Bytes::from(file))
            .map_err(|e| e.to_string())?;
        assert_eq!(file.metadata().file_metadata().num_rows(), 3);
        assert_eq!(file.metadata().num_row_groups(), 3, "a row group per page");
        Ok(())
    }

    /// The ZIP's first member starts before its next page is read, and the archive, written
    /// with data descriptors, reads back whole.
    #[cfg(feature = "tabular")]
    #[tokio::test]
    async fn a_zip_leaves_before_its_last_page_is_read() -> Result<(), String> {
        use std::io::Read as _;
        let gate = file::Gate::new(2);
        let slot = gate.take().map_err(|p| p.1.to_owned())?;
        let (pages, release, read) = held(1);
        let member = bundle::Member {
            path: "Car.csv".into(),
            extension: "csv",
            sink: csv_sink(1 << 20).map_err(|e| e.to_string())?,
            pages,
            provenance: Vec::new(),
            counted: true,
        };
        let sent = file::Sent {
            media: "application/zip",
            name: "city.zip",
            trailer: None,
        };
        let close = |rows: usize, files: &[(String, &str)]| {
            vec![(
                "README.md".to_owned(),
                format!("{rows} rows in {}", files.len()).into_bytes(),
            )]
        };
        let archive = antares_export::bundle::Bundle::new(1 << 20);
        let reply = file::streamed(&sent, slot, SECOND, move |tx| {
            bundle::members_into(tx, archive, vec![member], close)
        })
        .await;
        let mut body = reply.into_body();
        let Some(Ok(first)) = frame(&mut body).await? else {
            return Err("no first frame".into());
        };
        assert!(
            first.starts_with(b"PK\x03\x04"),
            "member 1's header leaves first"
        );
        assert_eq!(
            read.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "page 2 waits"
        );
        release.add_permits(10);
        let (more, error) = rest(&mut body).await?;
        assert_eq!(error, None);
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new([first, more].concat()))
            .map_err(|e| e.to_string())?;
        let mut csv = String::new();
        zip.by_name("Car.csv")
            .map_err(|e| e.to_string())?
            .read_to_string(&mut csv)
            .map_err(|e| e.to_string())?;
        assert_eq!(csv, "id\r\nurn:a\r\nurn:b\r\nurn:c\r\n");
        let mut readme = String::new();
        zip.by_name("README.md")
            .map_err(|e| e.to_string())?
            .read_to_string(&mut readme)
            .map_err(|e| e.to_string())?;
        assert_eq!(readme, "3 rows in 1");
        Ok(())
    }

    /// A file of exactly its byte cap is whole; one byte under, the page that crosses ends the
    /// transfer broken after a trailer naming why; a first page past it is a 413.
    #[cfg(feature = "tabular")]
    #[tokio::test]
    async fn a_file_is_whole_at_its_byte_cap_and_cut_past_it() -> Result<(), String> {
        // "id\r\n" and three rows of "urn:x\r\n": 4 + 3 * 7 bytes
        let at = file::Gate::with_limits(1, 25, SECOND);
        let (pages, release, _) = held(3);
        release.add_permits(1);
        let reply = csv_file(&at, pages, SECOND).await?;
        let (bytes, error) = rest(&mut reply.into_body()).await?;
        assert_eq!((bytes.len(), error), (25, None), "at the cap");
        let under = file::Gate::with_limits(1, 24, SECOND);
        let (pages, release, _) = held(3);
        release.add_permits(1);
        let reply = csv_file(&under, pages, SECOND).await?;
        assert_eq!(reply.status(), axum::http::StatusCode::OK);
        let (bytes, error) = rest(&mut reply.into_body()).await?;
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.starts_with(
                "id\r\nurn:a\r\nurn:b\r\n# the file stopped before its end: the file is larger"
            ),
            "{text}"
        );
        assert!(!text.contains("urn:c"), "{text}");
        assert!(error.is_some_and(|e| e.contains("larger than a file may be")));
        assert!(under.take().is_ok(), "a cut file frees its place");
        let tiny = file::Gate::with_limits(1, 8, SECOND);
        let (pages, _, _) = held(3);
        let reply = csv_file(&tiny, pages, SECOND).await?;
        assert_eq!(reply.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
        Ok(())
    }

    /// A file still read when its time runs out is cut after a trailer naming why; one whose
    /// first page is not read by then is a 503. Either way its place is free again.
    #[cfg(feature = "tabular")]
    #[tokio::test]
    async fn a_file_past_its_time_is_cut() -> Result<(), String> {
        let short = std::time::Duration::from_millis(150);
        let gate = file::Gate::with_limits(1, 1 << 20, short);
        let (pages, _never, _) = held(1);
        let started = std::time::Instant::now();
        let reply = csv_file(&gate, pages, short).await?;
        assert_eq!(reply.status(), axum::http::StatusCode::OK);
        let (bytes, error) = rest(&mut reply.into_body()).await?;
        assert!(started.elapsed() < SECOND, "{:?}", started.elapsed());
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.ends_with("# the file stopped before its end: the file took longer than a file may take to write: narrow it (bbox, datetime, filter)\r\n"), "{text}");
        assert!(error.is_some_and(|e| e.contains("took longer")));
        assert!(gate.take().is_ok(), "a cut file frees its place");
        let (pages, _never, _) = held(0);
        let reply = csv_file(&gate, pages, short).await?;
        assert_eq!(reply.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
        Ok(())
    }

    /// Pages of `rows` Features each, each read after `wait` of (paused) time: a store page
    /// read at the speed a loaded host measured.
    #[cfg(all(feature = "tabular", feature = "parquet"))]
    struct Slow {
        left: usize,
        rows: usize,
        wait: std::time::Duration,
    }

    #[cfg(all(feature = "tabular", feature = "parquet"))]
    impl file::Pages for Slow {
        fn next(
            &mut self,
        ) -> futures_util::future::BoxFuture<
            '_,
            Result<Option<Vec<serde_json::Value>>, crate::Problem>,
        > {
            Box::pin(async move {
                if self.left == 0 {
                    return Ok(None);
                }
                self.left = self.left.saturating_sub(1);
                tokio::time::sleep(self.wait).await;
                Ok(Some(
                    (0..self.rows)
                        .map(|n| serde_json::json!({"id": format!("urn:{n}"), "properties": {}}))
                        .collect(),
                ))
            })
        }
    }

    /// The prod Garden ZIP (jc-garden-zip): 256 pages read for its CSV and again for its
    /// Parquet at the 0.31 s a page the loaded restore copy took, 159 s, comes out whole within
    /// the default file time; the old 120 s cut it in its Parquet.
    #[cfg(all(feature = "tabular", feature = "parquet"))]
    #[tokio::test(start_paused = true)]
    async fn the_prod_garden_zip_on_a_loaded_host_is_whole() -> Result<(), String> {
        use parquet::file::reader::FileReader as _;
        use std::io::Read as _;
        const PAGES: usize = 256;
        const ROWS: usize = 10;
        let gate = file::Gate::new(1);
        let slot = gate.take().map_err(|p| p.1.to_owned())?;
        let garden = || -> Box<dyn file::Pages> {
            Box::new(Slow {
                left: PAGES,
                rows: ROWS,
                wait: std::time::Duration::from_millis(310),
            })
        };
        let members = vec![
            bundle::Member {
                path: "Garden.csv".into(),
                extension: "csv",
                sink: csv_sink(gate.bytes).map_err(|e| e.to_string())?,
                pages: garden(),
                provenance: Vec::new(),
                counted: true,
            },
            bundle::Member {
                path: "Garden.parquet".into(),
                extension: "parquet",
                sink: file::parquet("Garden", Vec::new(), false, gate.bytes)
                    .map_err(|e| e.to_string())?,
                pages: garden(),
                provenance: Vec::new(),
                counted: false,
            },
        ];
        let sent = file::Sent {
            media: "application/zip",
            name: "praha-zelen.zip",
            trailer: None,
        };
        let close = |_: usize, _: &[(String, &str)]| Vec::new();
        let archive = antares_export::bundle::Bundle::new(gate.bytes);
        let time = gate.time;
        let reply = file::streamed(&sent, slot, time, move |tx| {
            bundle::members_into(tx, archive, members, close)
        })
        .await;
        // A deflated member sends nothing for many pages: read the body whole, not frame by frame.
        let bytes = http_body_util::BodyExt::collect(reply.into_body())
            .await
            .map_err(|e| format!("the ZIP was cut: {e}"))?
            .to_bytes();
        let mut zip =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let mut csv = String::new();
        zip.by_name("Garden.csv")
            .map_err(|e| e.to_string())?
            .read_to_string(&mut csv)
            .map_err(|e| e.to_string())?;
        assert_eq!(
            csv.lines().count(),
            PAGES * ROWS + 1,
            "every row and the header"
        );
        let mut parquet = Vec::new();
        zip.by_name("Garden.parquet")
            .map_err(|e| e.to_string())?
            .read_to_end(&mut parquet)
            .map_err(|e| e.to_string())?;
        let parquet = parquet::file::reader::SerializedFileReader::new(bytes::Bytes::from(parquet))
            .map_err(|e| e.to_string())?;
        let rows = usize::try_from(parquet.metadata().file_metadata().num_rows())
            .map_err(|e| e.to_string())?;
        assert_eq!(rows, PAGES * ROWS);
        Ok(())
    }

    /// Where a test's log lines go.
    #[cfg(feature = "tabular")]
    #[derive(Clone, Default)]
    struct Log(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    #[cfg(feature = "tabular")]
    impl std::io::Write for Log {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if let Ok(mut held) = self.0.lock() {
                held.extend_from_slice(buf);
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[cfg(feature = "tabular")]
    impl Log {
        fn text(&self) -> String {
            self.0
                .lock()
                .map(|held| String::from_utf8_lossy(&held).into_owned())
                .unwrap_or_default()
        }
    }

    /// A file cut by its time, and one cut by its bytes, each log one WARN under the request's
    /// span naming why, the rows and bytes written and the seconds; a whole file logs none.
    #[cfg(feature = "tabular")]
    #[tokio::test]
    async fn a_cut_file_logs_one_warn_with_its_request() -> Result<(), String> {
        // The process's subscriber, not a scoped one: a callsite a parallel test hits first
        // under no subscriber stays off for a scoped one. Lines are told apart by request id.
        static LOG: std::sync::OnceLock<Log> = std::sync::OnceLock::new();
        let log = LOG.get_or_init(|| {
            let log = Log::default();
            let writer = log.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(move || writer.clone())
                .with_ansi(false)
                .finish();
            let _ = tracing::subscriber::set_global_default(subscriber);
            log
        });
        let warns = |id: &str| -> Vec<String> {
            log.text()
                .lines()
                .filter(|l| l.contains("WARN") && l.contains(id))
                .map(str::to_owned)
                .collect()
        };
        let short = std::time::Duration::from_millis(150);
        let gate = file::Gate::with_limits(1, 1 << 20, short);
        let (pages, _never, _) = held(1);
        let span = tracing::info_span!("request", request_id = "garden-zip-time");
        let reply = tracing::Instrument::instrument(csv_file(&gate, pages, short), span).await?;
        let _ = rest(&mut reply.into_body()).await?;
        let lines = warns("garden-zip-time");
        assert_eq!(lines.len(), 1, "{}", log.text());
        let line = lines.first().cloned().unwrap_or_default();
        for part in [
            "an OGC file stopped before its end",
            "took longer than a file may take",
            "file=\"Car.csv\"",
            "rows=1",
            "bytes=11",
            "seconds=0.1",
        ] {
            assert!(line.contains(part), "{part}: {line}");
        }
        let under = file::Gate::with_limits(1, 24, SECOND);
        let (pages, release, _) = held(3);
        release.add_permits(1);
        let span = tracing::info_span!("request", request_id = "garden-zip-bytes");
        let reply = tracing::Instrument::instrument(csv_file(&under, pages, SECOND), span).await?;
        let _ = rest(&mut reply.into_body()).await?;
        let lines = warns("garden-zip-bytes");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines
                .iter()
                .all(|l| l.contains("larger than a file may be") && l.contains("rows=")),
            "{lines:?}"
        );
        let whole = file::Gate::with_limits(1, 1 << 20, SECOND);
        let (pages, release, _) = held(3);
        release.add_permits(1);
        let span = tracing::info_span!("request", request_id = "garden-zip-whole");
        let reply = tracing::Instrument::instrument(csv_file(&whole, pages, SECOND), span).await?;
        let _ = rest(&mut reply.into_body()).await?;
        assert_eq!(warns("garden-zip-whole"), Vec::<String>::new());
        Ok(())
    }

    /// A client that leaves mid-file drops the body: the task writing it stops and its place
    /// is free again at once, while the read it waited on never finished.
    #[cfg(feature = "tabular")]
    #[tokio::test]
    async fn a_file_whose_client_leaves_frees_its_place() -> Result<(), String> {
        let gate = file::Gate::with_limits(1, 1 << 20, std::time::Duration::from_secs(60));
        let (pages, _never, _) = held(1);
        let reply = csv_file(&gate, pages, std::time::Duration::from_secs(60)).await?;
        let mut body = reply.into_body();
        assert!(matches!(frame(&mut body).await?, Some(Ok(_))));
        assert_eq!(
            gate.take().err(),
            Some(file::BUSY),
            "the file holds its place"
        );
        drop(body);
        for _ in 0..100 {
            if gate.take().is_ok() {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        Err("the place was not freed within a second".into())
    }
}
