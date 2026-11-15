use std::collections::VecDeque;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use ekiben_bloblang::{Executor, Value};
use ekiben_core::config::{parse_duration_str, BatchPolicyConfig};
use ekiben_core::{
    AckResponder, Batch, Batcher, ComponentConfig, CoreError, Input, Part, PartMetaAdapter,
    PartsView, Registry, StreamContext,
};
use serde_json::Value as JsonValue;
use tokio::sync::Mutex as AsyncMutex;

use crate::path_guard::sanitize_file_path;

pub fn register(registry: &Registry) {
    registry.register_input("generate", |conf, ctx| {
        Ok(Box::new(GenerateInput::from_config(conf, ctx)?))
    });

    registry.register_input("file", |conf, ctx| {
        Ok(Box::new(FileInput::from_config(conf, ctx)?))
    });

    registry.register_input("csv", |conf, ctx| {
        Ok(Box::new(FileInput::from_csv_config(conf, ctx)?))
    });

    registry.register_input("broker", |conf, ctx| {
        Ok(Box::new(BrokerInput::from_config(conf, ctx)?))
    });

    registry.register_input("batched", |conf, ctx| {
        Ok(Box::new(BatchedInput::from_config(conf, ctx)?))
    });

    registry.register_input("sequence", |conf, ctx| {
        Ok(Box::new(SequenceInput::from_config(conf, ctx)?))
    });

    registry.register_input("read_until", |conf, ctx| {
        Ok(Box::new(ReadUntilInput::from_config(conf, ctx)?))
    });

    registry.register_input("resource", |conf, _ctx| {
        let name = conf
            .config
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::config("resource input requires a resource name"))?
            .to_string();
        Ok(Box::new(ResourceInput { name }))
    });

    registry.register_input("inproc", |_conf, _ctx| Ok(Box::new(EmptyInput)));
    registry.register_input("stdin", |conf, ctx| {
        Ok(Box::new(StdinInput::from_config(conf, ctx)?))
    });
}

/// Implements `generate` input (`internal/impl/pure/input_generate.go`).
pub struct GenerateInput {
    exec: Executor,
    timer: GenerateTimer,
    remaining: Option<usize>,
    batch_size: usize,
    first_run: bool,
    replay: Option<Arc<NackReplay>>,
}

/// How `interval` paces the reads: a duration ticks after a free first batch, a cron schedule
/// waits for its first activation (Bento's `firstIsFree: !isCron`).
enum GenerateTimer {
    None,
    Every(Duration),
    Cron(crate::cron::Schedule),
}

/// Nacked batches waiting to be read again, and how many sent batches are still unanswered:
/// a `count`-limited input ends only once every batch it sent is acked.
#[derive(Default)]
struct NackReplay {
    state: std::sync::Mutex<(VecDeque<Batch>, usize)>,
    changed: tokio::sync::Notify,
}

impl NackReplay {
    fn lock(&self) -> std::sync::MutexGuard<'_, (VecDeque<Batch>, usize)> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn track(self: &Arc<Self>, batch: &Batch) -> AckResponder {
        self.lock().1 += 1;
        // The guard answers for the batch when its responder is dropped unanswered too.
        let guard = InFlight(self.clone());
        let copy = batch.clone();
        AckResponder::from_fn(move |res| {
            if res.is_some() {
                guard.0.lock().0.push_back(copy);
            }
            drop(guard);
        })
    }

    /// A nacked batch to send again, waiting for the unanswered ones when `drain` is set.
    async fn next(&self, drain: bool) -> Option<Batch> {
        loop {
            let notified = self.changed.notified();
            {
                let mut st = self.lock();
                if let Some(b) = st.0.pop_front() {
                    return Some(b);
                }
                if !drain || st.1 == 0 {
                    return None;
                }
            }
            notified.await;
        }
    }
}

struct InFlight(Arc<NackReplay>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.lock().1 -= 1;
        self.0.changed.notify_one();
    }
}

/// Resolves once the stream is told to stop.
async fn stopped(ctx: &StreamContext) {
    let mut rx = ctx.shutdown_rx.clone();
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

impl GenerateInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("generate input config must be an object"))?;

        let mapping_str = obj
            .get("mapping")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("generate input requires a 'mapping' field"))?;
        let exec = ctx.blobl_env.compile_mapping(mapping_str)?;

        // Bento's default is `1s`; an empty string generates as fast as the stream reads.
        let interval = match obj.get("interval") {
            None => "1s",
            Some(v) => v
                .as_str()
                .ok_or_else(|| CoreError::config("generate input 'interval' must be a string"))?,
        };
        let timer = generate_timer(interval)?;

        let count = obj.get("count").and_then(JsonValue::as_u64).unwrap_or(0) as usize;
        let remaining = if count > 0 { Some(count) } else { None };
        let batch_size = obj
            .get("batch_size")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1)
            .max(1) as usize;
        let auto_replay_nacks = obj
            .get("auto_replay_nacks")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true);

        Ok(Self {
            exec,
            timer,
            remaining,
            batch_size,
            first_run: true,
            replay: auto_replay_nacks.then(Arc::default),
        })
    }

    fn ack_for(&self, batch: &Batch) -> AckResponder {
        match &self.replay {
            Some(r) => r.track(batch),
            None => AckResponder::noop(),
        }
    }

    async fn wait_tick(&self, ctx: &StreamContext) -> Result<(), CoreError> {
        let wait = match &self.timer {
            GenerateTimer::None => return Ok(()),
            GenerateTimer::Every(_) if self.first_run => return Ok(()),
            GenerateTimer::Every(iv) => *iv,
            GenerateTimer::Cron(s) => {
                let now = Utc::now();
                let at = s.next(now).ok_or_else(|| {
                    CoreError::other("generate input: the cron interval never fires again")
                })?;
                (at - now).to_std().unwrap_or_default()
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => Ok(()),
            _ = stopped(ctx) => Err(CoreError::EndOfInput),
        }
    }
}

/// Bento reads `interval` as a Go duration first, then as a cron expression.
fn generate_timer(interval: &str) -> Result<GenerateTimer, CoreError> {
    if interval.is_empty() {
        return Ok(GenerateTimer::None);
    }
    let dur_err = match parse_duration_str(interval) {
        Ok(d) if d.is_zero() => return Ok(GenerateTimer::None),
        Ok(d) => return Ok(GenerateTimer::Every(d)),
        Err(e) => e.to_string(),
    };
    crate::cron::parse(interval)
        .map(GenerateTimer::Cron)
        .map_err(|cron_err| {
            CoreError::config(format!(
                "failed to parse interval as duration string: {}, or as cron expression: {cron_err}",
                dur_err.trim()
            ))
        })
}

#[async_trait]
impl Input for GenerateInput {
    fn input_type(&self) -> &'static str {
        "generate"
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        let exhausted = self.remaining == Some(0);
        if let Some(r) = self.replay.clone() {
            let replayed = tokio::select! {
                b = r.next(exhausted) => b,
                _ = stopped(ctx) => return Err(CoreError::EndOfInput),
            };
            if let Some(replayed) = replayed {
                let ack = self.ack_for(&replayed);
                return Ok((replayed, ack));
            }
        }
        if exhausted {
            return Err(CoreError::EndOfInput);
        }

        self.wait_tick(ctx).await?;
        self.first_run = false;

        let to_gen = match self.remaining {
            Some(rem) => {
                let n = rem.min(self.batch_size);
                self.remaining = Some(rem - n);
                n
            }
            None => self.batch_size,
        };

        let mut batch = Vec::with_capacity(to_gen);
        let empty_ref = [Part::default()];
        let view = PartsView(&empty_ref);
        for _ in 0..to_gen {
            let mut part = Part::default();
            let res = {
                let mut adapter = PartMetaAdapter(&mut part);
                self.exec.map_part(0, &view, &mut adapter, None)?
            };
            match res {
                None => {}
                Some(val) => {
                    part.apply_mapped_value(val);
                    batch.push(part);
                }
            }
        }

        let ack = self.ack_for(&batch);
        Ok((batch, ack))
    }
}

#[derive(Clone, Debug)]
enum FileScannerKind {
    Lines(crate::scanner_lines::LinesScanner),
    ToTheEnd,
    JsonDocuments,
    Csv(crate::scanner_csv::CsvScanner),
    Switch(Vec<SwitchCase>),
    Tar,
    Chunker {
        size: usize,
    },
    Decompress {
        algorithm: crate::scanner_decompress::Algorithm,
        into: Box<FileScannerKind>,
    },
    SkipBom {
        into: Box<FileScannerKind>,
    },
    ReMatch(crate::scanner_re_match::ReMatchScanner),
    XmlDocuments(crate::scanner_xml::XmlDocumentScanner),
}

/// One `switch` case: the child scanner of the first case whose `re_match_name` matches the
/// source's name (a file's path), a case without one matching every name.
#[derive(Clone, Debug)]
struct SwitchCase {
    name: Option<regex::Regex>,
    scanner: FileScannerKind,
}

/// Implements `file` and `csv` inputs (`internal/impl/io/input_file.go`): a reader thread scans
/// one file at a time into a bounded channel, so a stream holds a batch or two of a file, never
/// the whole file (the `to_the_end`, `tar`, `re_match` and `xml_documents` scanners excepted,
/// whose message is the whole document or entry).
pub struct FileInput {
    paths: VecDeque<PathBuf>,
    scanner: FileScannerKind,
    delete_on_finish: bool,
    batch_count: usize,
    current: Option<ScanStream>,
}

/// The batches of one source as its reader thread scans them, with the next one held back when
/// the input must know which batch is the last (`delete_on_finish`).
struct ScanStream {
    rx: tokio::sync::mpsc::Receiver<Result<Batch, CoreError>>,
    path: Option<PathBuf>,
    peeked: Option<Batch>,
}

impl ScanStream {
    /// Starts a thread that opens a source and sends its parts in batches of `batch_count`.
    fn spawn<R, F>(
        open: F,
        name: String,
        scanner: FileScannerKind,
        batch_count: usize,
        path: Option<PathBuf>,
    ) -> Self
    where
        R: std::io::Read,
        F: FnOnce() -> Result<(R, Box<dyn Fn(&mut Part) + Send>), CoreError> + Send + 'static,
    {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        std::thread::spawn(move || {
            let res = open().and_then(|(src, annotate)| {
                let mut reader = std::io::BufReader::with_capacity(SCAN_BUFFER, src);
                let mut batch = Vec::with_capacity(batch_count);
                scan_reader(&mut reader, &scanner, &name, &mut |mut p| {
                    annotate(&mut p);
                    batch.push(p);
                    if batch.len() >= batch_count {
                        let full = std::mem::replace(&mut batch, Vec::with_capacity(batch_count));
                        tx.blocking_send(Ok(full))
                            .map_err(|_| CoreError::other("the stream stopped reading"))?;
                    }
                    Ok(())
                })?;
                if !batch.is_empty() {
                    let _ = tx.blocking_send(Ok(batch));
                }
                Ok(())
            });
            if let Err(e) = res {
                let _ = tx.blocking_send(Err(e));
            }
        });
        Self {
            rx,
            path,
            peeked: None,
        }
    }

    async fn recv(&mut self) -> Result<Option<Batch>, CoreError> {
        if let Some(b) = self.peeked.take() {
            return Ok(Some(b));
        }
        self.rx.recv().await.transpose()
    }
}

/// Read buffer of a scanned source; a line or row may span several fills.
const SCAN_BUFFER: usize = 64 * 1024;

impl FileInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("file input config must be an object"))?;

        let raw_paths: Vec<String> = match obj.get("paths") {
            Some(JsonValue::Array(arr)) => arr
                .iter()
                .filter_map(JsonValue::as_str)
                .map(String::from)
                .collect(),
            Some(JsonValue::String(s)) => vec![s.clone()],
            _ => return Err(CoreError::config("file input requires a 'paths' field")),
        };

        let paths = expand_paths(&raw_paths, ctx.file_root.as_deref())?;
        let delete_on_finish = obj
            .get("delete_on_finish")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);

        let scanner = parse_scanner_config(obj)?;

        Ok(Self {
            paths,
            scanner,
            delete_on_finish,
            batch_count: 1,
            current: None,
        })
    }

    pub fn from_csv_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("csv input config must be an object"))?;
        let raw_paths: Vec<String> = match obj.get("paths") {
            Some(JsonValue::Array(arr)) => arr
                .iter()
                .filter_map(JsonValue::as_str)
                .map(String::from)
                .collect(),
            _ => return Err(CoreError::config("csv input requires a 'paths' field")),
        };
        let paths = expand_paths(&raw_paths, ctx.file_root.as_deref())?;
        let csv = crate::scanner_csv::CsvScanner::from_config(obj, "delimiter")?;
        let delete_on_finish = obj
            .get("delete_on_finish")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let batch_count = obj
            .get("batch_count")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1)
            .max(1) as usize;

        Ok(Self {
            paths,
            scanner: FileScannerKind::Csv(csv),
            delete_on_finish,
            batch_count,
            current: None,
        })
    }

    fn open_next_file(&mut self) -> Option<ScanStream> {
        let path = self.paths.pop_front()?;
        let for_read = path.clone();
        let open = move || {
            let file = std::fs::File::open(&for_read).map_err(|e| {
                CoreError::other(format!("failed to read '{}': {e}", for_read.display()))
            })?;
            let metadata = file.metadata().map_err(|e| {
                CoreError::other(format!("failed to stat '{}': {e}", for_read.display()))
            })?;
            let mod_time: DateTime<Utc> = metadata
                .modified()
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(|_| Utc::now());
            let mod_unix = mod_time.timestamp();
            let mod_rfc3339 = mod_time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let path_str = for_read.to_string_lossy().into_owned();
            let annotate: Box<dyn Fn(&mut Part) + Send> = Box::new(move |p: &mut Part| {
                p.meta_set_str("path", path_str.clone());
                p.meta_set("mod_time_unix", Value::Int(mod_unix));
                p.meta_set_str("mod_time", mod_rfc3339.clone());
            });
            Ok((file, annotate))
        };
        let name = path.to_string_lossy().into_owned();
        Some(ScanStream::spawn(
            open,
            name,
            self.scanner.clone(),
            self.batch_count,
            Some(path),
        ))
    }
}

#[async_trait]
impl Input for FileInput {
    fn input_type(&self) -> &'static str {
        "file"
    }

    async fn read_batch(
        &mut self,
        _ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            if self.current.is_none() {
                self.current = Some(self.open_next_file().ok_or(CoreError::EndOfInput)?);
            }
            let Some(cur) = self.current.as_mut() else {
                continue;
            };
            let Some(batch) = cur.recv().await? else {
                self.current = None;
                continue;
            };
            if !self.delete_on_finish {
                return Ok((batch, AckResponder::noop()));
            }
            // The file goes once its last batch is acked: look one batch ahead to know it.
            match cur.recv().await? {
                Some(next) => {
                    cur.peeked = Some(next);
                    return Ok((batch, AckResponder::noop()));
                }
                None => {
                    let path = cur.path.take();
                    self.current = None;
                    let ack = AckResponder::from_fn(move |err| {
                        if let (None, Some(p)) = (err, path) {
                            let _ = std::fs::remove_file(p);
                        }
                    });
                    return Ok((batch, ack));
                }
            }
        }
    }
}

/// Splits a source into message parts with a file/stdin scanner, calling `emit` with each part
/// as it is read.
fn scan_reader(
    r: &mut dyn std::io::BufRead,
    scanner: &FileScannerKind,
    name: &str,
    emit: &mut dyn FnMut(Part) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    use std::io::Read;
    let io = |e: std::io::Error| CoreError::other(e.to_string());
    let read_all = |r: &mut dyn std::io::BufRead| -> Result<Vec<u8>, CoreError> {
        let mut raw = Vec::new();
        r.read_to_end(&mut raw).map_err(io)?;
        Ok(raw)
    };
    match scanner {
        FileScannerKind::Decompress { algorithm, into } => {
            let mut inner = std::io::BufReader::with_capacity(SCAN_BUFFER, algorithm.reader(r)?);
            scan_reader(&mut inner, into, name, emit)
        }
        FileScannerKind::SkipBom { into } => {
            let mut head = Vec::with_capacity(4);
            r.take(4).read_to_end(&mut head).map_err(io)?;
            let rest = crate::scanner_skip_bom::skip_bom(&head).to_vec();
            let mut inner =
                std::io::BufReader::with_capacity(SCAN_BUFFER, Cursor::new(rest).chain(r));
            scan_reader(&mut inner, into, name, emit)
        }
        FileScannerKind::Switch(cases) => {
            let case = cases
                .iter()
                .find(|c| c.name.as_ref().is_none_or(|re| re.is_match(name)))
                .ok_or_else(|| {
                    CoreError::other("source details did not match against any scanners")
                })?;
            scan_reader(r, &case.scanner, name, emit)
        }
        FileScannerKind::ReMatch(re) => re
            .split(&read_all(r)?)?
            .into_iter()
            .try_for_each(|b| emit(Part::new(b))),
        FileScannerKind::XmlDocuments(xml) => xml
            .documents(&read_all(r)?)?
            .into_iter()
            .try_for_each(|v| emit(Part::from_structured(v))),
        FileScannerKind::ToTheEnd => emit(Part::new(Bytes::from(read_all(r)?))),
        FileScannerKind::Lines(lines) => lines.scan(r, &mut |b| emit(Part::new(b))),
        FileScannerKind::JsonDocuments => {
            for doc in serde_json::Deserializer::from_reader(r).into_iter::<JsonValue>() {
                emit(Part::from_json(
                    doc.map_err(|e| CoreError::other(e.to_string()))?,
                ))?;
            }
            Ok(())
        }
        FileScannerKind::Csv(csv) => csv.scan(r, emit),
        FileScannerKind::Tar => {
            let mut arch = tar::Archive::new(r);
            for entry in arch.entries().map_err(io)? {
                let mut buf = Vec::new();
                entry.map_err(io)?.read_to_end(&mut buf).map_err(io)?;
                emit(Part::new(Bytes::from(buf)))?;
            }
            Ok(())
        }
        FileScannerKind::Chunker { size } => loop {
            let mut buf = Vec::with_capacity(*size);
            (&mut *r)
                .take(*size as u64)
                .read_to_end(&mut buf)
                .map_err(io)?;
            if buf.is_empty() {
                return Ok(());
            }
            emit(Part::new(Bytes::from(buf)))?;
        },
    }
}

fn expand_paths(
    patterns: &[String],
    root_opt: Option<&std::path::Path>,
) -> Result<VecDeque<PathBuf>, CoreError> {
    let mut out = VecDeque::new();
    for pat in patterns {
        let safe_pat = sanitize_file_path(pat, root_opt)?;
        let pat_str = safe_pat.to_string_lossy();
        if pat_str.contains('*') || pat_str.contains('?') || pat_str.contains('[') {
            let entries = glob::glob(&pat_str)
                .map_err(|e| CoreError::config(format!("invalid glob pattern '{pat}': {e}")))?;
            for entry in entries.flatten() {
                out.push_back(entry);
            }
        } else {
            out.push_back(safe_pat);
        }
    }
    Ok(out)
}

fn parse_scanner_config(
    obj: &serde_json::Map<String, JsonValue>,
) -> Result<FileScannerKind, CoreError> {
    if let Some(codec) = obj.get("codec").and_then(JsonValue::as_str) {
        return match codec {
            "all-bytes" | "to_the_end" => Ok(FileScannerKind::ToTheEnd),
            "csv" => Ok(FileScannerKind::Csv(Default::default())),
            "tar" => Ok(FileScannerKind::Tar),
            _ => Ok(FileScannerKind::Lines(Default::default())),
        };
    }
    match obj.get("scanner").and_then(JsonValue::as_object) {
        Some(scanner_obj) => parse_scanner(scanner_obj),
        None => Ok(FileScannerKind::Lines(Default::default())),
    }
}

/// `switch`: a list of `{re_match_name, scanner}` cases (`internal/impl/pure/scanner_switch.go`).
fn parse_switch(cases: &JsonValue) -> Result<FileScannerKind, CoreError> {
    const HINT: &str =
        "switch scanner takes a list of cases, for example [{re_match_name: '\\.csv$', scanner: {csv: {}}}]";
    let cases = cases.as_array().ok_or_else(|| CoreError::config(HINT))?;
    cases
        .iter()
        .map(|case| {
            let obj = case.as_object().ok_or_else(|| CoreError::config(HINT))?;
            let name = match obj.get("re_match_name").and_then(JsonValue::as_str) {
                None | Some("") => None,
                Some(pat) => Some(
                    regex::RegexBuilder::new(pat)
                        .size_limit(1 << 20)
                        .build()
                        .map_err(|e| {
                            CoreError::config(format!("switch: invalid re_match_name {pat:?}: {e}"))
                        })?,
                ),
            };
            let scanner = obj
                .get("scanner")
                .and_then(JsonValue::as_object)
                .ok_or_else(|| CoreError::config("switch: each case needs a scanner"))
                .and_then(parse_scanner)?;
            Ok(SwitchCase { name, scanner })
        })
        .collect::<Result<_, _>>()
        .map(FileScannerKind::Switch)
}

/// One scanner object (`{<name>: {..}}`), nested ones (`into`) included.
fn parse_scanner(
    scanner_obj: &serde_json::Map<String, JsonValue>,
) -> Result<FileScannerKind, CoreError> {
    let empty = serde_json::Map::new();
    let field = |name: &str| {
        scanner_obj
            .get(name)
            .map(|v| v.as_object().unwrap_or(&empty))
    };
    let child = |conf: &serde_json::Map<String, JsonValue>| match conf
        .get("into")
        .and_then(JsonValue::as_object)
    {
        Some(into) => parse_scanner(into).map(Box::new),
        None => Ok(Box::new(FileScannerKind::ToTheEnd)),
    };
    if let Some(conf) = field("decompress") {
        let name = conf
            .get("algorithm")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("field algorithm is required"))?;
        return Ok(FileScannerKind::Decompress {
            algorithm: crate::scanner_decompress::Algorithm::parse(name)?,
            into: child(conf)?,
        });
    }
    if let Some(conf) = field("skip_bom") {
        return Ok(FileScannerKind::SkipBom { into: child(conf)? });
    }
    if let Some(conf) = field("re_match") {
        return Ok(FileScannerKind::ReMatch(
            crate::scanner_re_match::ReMatchScanner::from_config(conf)?,
        ));
    }
    if let Some(conf) = field("xml_documents") {
        return Ok(FileScannerKind::XmlDocuments(
            crate::scanner_xml::XmlDocumentScanner::from_config(conf)?,
        ));
    }
    if scanner_obj.contains_key("to_the_end") {
        return Ok(FileScannerKind::ToTheEnd);
    }
    if scanner_obj.contains_key("json_documents") {
        return Ok(FileScannerKind::JsonDocuments);
    }
    if scanner_obj.contains_key("tar") {
        return Ok(FileScannerKind::Tar);
    }
    if let Some(ch_obj) = scanner_obj.get("chunker").and_then(JsonValue::as_object) {
        let size = ch_obj
            .get("size")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1024) as usize;
        return Ok(FileScannerKind::Chunker { size });
    }
    if let Some(conf) = field("csv") {
        return Ok(FileScannerKind::Csv(
            crate::scanner_csv::CsvScanner::from_config(conf, "custom_delimiter")?,
        ));
    }
    if let Some(conf) = field("lines") {
        return Ok(FileScannerKind::Lines(
            crate::scanner_lines::LinesScanner::from_config(conf)?,
        ));
    }
    if let Some(cases) = scanner_obj.get("switch") {
        return parse_switch(cases);
    }
    // A scanner ekiben does not build is refused, never read as lines.
    let name = scanner_obj.keys().next().map_or("", String::as_str);
    Err(CoreError::config(format!(
        "scanner type '{name}' was not recognised"
    )))
}

/// Implements `broker` input (`internal/impl/pure/input_broker.go`),
/// combining multiple inputs in parallel (or round-robin) with an optional `batching` policy.
pub struct BrokerInput {
    inputs: Vec<Box<dyn Input>>,
    cursor: usize,
    batcher: Option<Batcher>,
    pending_acks: Vec<AckResponder>,
}

impl BrokerInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("broker input config must be an object"))?;

        let copies = obj
            .get("copies")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1)
            .max(1) as usize;

        let in_arr = obj
            .get("inputs")
            .and_then(JsonValue::as_array)
            .filter(|a| !a.is_empty())
            .ok_or_else(|| {
                CoreError::config("attempting to create broker input type with no inputs")
            })?;

        let mut inputs = Vec::with_capacity(in_arr.len() * copies);
        for _ in 0..copies {
            for item in in_arr {
                let c = ComponentConfig::from_json_value(item, "stdin", None)?;
                inputs.push(ctx.registry.build_input(&c, ctx)?);
            }
        }

        let batcher = match obj.get("batching") {
            Some(b_val) => {
                let pol = BatchPolicyConfig::from_json_value(b_val)?;
                if pol.is_noop() && pol.processors.is_empty() && pol.count <= 1 {
                    None
                } else {
                    let pctx =
                        ctx.to_proc_context("broker", conf.label.clone(), "input.broker.batching");
                    Some(Batcher::new(&pol, pctx)?)
                }
            }
            None => None,
        };

        Ok(Self {
            inputs,
            cursor: 0,
            batcher,
            pending_acks: Vec::new(),
        })
    }
}

#[async_trait]
impl Input for BrokerInput {
    fn input_type(&self) -> &'static str {
        "broker"
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        for inp in &mut self.inputs {
            inp.connect(ctx).await?;
        }
        Ok(())
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            if self.inputs.is_empty() {
                if let Some(ref mut batcher) = self.batcher {
                    if let Some(flushed) = batcher.flush().await? {
                        let acks = std::mem::take(&mut self.pending_acks);
                        return Ok((
                            flushed,
                            AckResponder::from_fn(move |err| {
                                for a in acks {
                                    a.respond(err.clone());
                                }
                            }),
                        ));
                    }
                }
                return Err(CoreError::EndOfInput);
            }

            let idx = self.cursor % self.inputs.len();
            match self.inputs[idx].read_batch(ctx).await {
                Ok((batch, ack)) => {
                    self.cursor = (idx + 1) % self.inputs.len();
                    if let Some(ref mut batcher) = self.batcher {
                        self.pending_acks.push(ack);
                        let mut should_flush = false;
                        for part in batch {
                            if batcher.add(part) {
                                should_flush = true;
                            }
                        }
                        if should_flush {
                            if let Some(flushed) = batcher.flush().await? {
                                let acks = std::mem::take(&mut self.pending_acks);
                                return Ok((
                                    flushed,
                                    AckResponder::from_fn(move |err| {
                                        for a in acks {
                                            a.respond(err.clone());
                                        }
                                    }),
                                ));
                            }
                        }
                        continue;
                    } else {
                        return Ok((batch, ack));
                    }
                }
                Err(CoreError::EndOfInput) => {
                    let mut finished = self.inputs.remove(idx);
                    let _ = finished.close().await;
                    if self.cursor >= self.inputs.len() && !self.inputs.is_empty() {
                        self.cursor = 0;
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn is_connected(&self) -> bool {
        self.inputs.iter().all(|i| i.is_connected())
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        for inp in &mut self.inputs {
            let _ = inp.close().await;
        }
        Ok(())
    }
}

pub struct BatchedInput {
    child: Box<dyn Input>,
    batcher: Batcher,
    pending_acks: Vec<AckResponder>,
}

impl BatchedInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("batched input config must be an object"))?;
        let child_val = obj
            .get("child")
            .ok_or_else(|| CoreError::config("batched input requires a child field"))?;
        let child_conf = ComponentConfig::from_json_value(child_val, "stdin", None)?;
        let child = ctx.registry.build_input(&child_conf, ctx)?;
        let pol_val = obj
            .get("policy")
            .ok_or_else(|| CoreError::config("batched input requires a policy field"))?;
        let pol = BatchPolicyConfig::from_json_value(pol_val)?;
        let pctx = ctx.to_proc_context("batched", conf.label.clone(), "input.batched.policy");
        let batcher = Batcher::new(&pol, pctx)?;
        Ok(Self {
            child,
            batcher,
            pending_acks: Vec::new(),
        })
    }
}

#[async_trait]
impl Input for BatchedInput {
    fn input_type(&self) -> &'static str {
        "batched"
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        self.child.connect(ctx).await
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            match self.child.read_batch(ctx).await {
                Ok((batch, ack)) => {
                    self.pending_acks.push(ack);
                    let mut flush = false;
                    for p in batch {
                        if self.batcher.add(p) {
                            flush = true;
                        }
                    }
                    if flush {
                        if let Some(flushed) = self.batcher.flush().await? {
                            let acks = std::mem::take(&mut self.pending_acks);
                            return Ok((
                                flushed,
                                AckResponder::from_fn(move |err| {
                                    for a in acks {
                                        a.respond(err.clone());
                                    }
                                }),
                            ));
                        }
                    }
                }
                Err(CoreError::EndOfInput) => {
                    if let Some(flushed) = self.batcher.flush().await? {
                        let acks = std::mem::take(&mut self.pending_acks);
                        return Ok((
                            flushed,
                            AckResponder::from_fn(move |err| {
                                for a in acks {
                                    a.respond(err.clone());
                                }
                            }),
                        ));
                    }
                    return Err(CoreError::EndOfInput);
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        self.child.close().await
    }
}

pub struct SequenceInput {
    inputs: VecDeque<Box<dyn Input>>,
}

impl SequenceInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let arr = conf
            .config
            .get("inputs")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| CoreError::config("sequence input requires an 'inputs' array"))?;
        let mut inputs = VecDeque::with_capacity(arr.len());
        for item in arr {
            let c = ComponentConfig::from_json_value(item, "stdin", None)?;
            inputs.push_back(ctx.registry.build_input(&c, ctx)?);
        }
        Ok(Self { inputs })
    }
}

#[async_trait]
impl Input for SequenceInput {
    fn input_type(&self) -> &'static str {
        "sequence"
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        if let Some(front) = self.inputs.front_mut() {
            front.connect(ctx).await?;
        }
        Ok(())
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            let Some(front) = self.inputs.front_mut() else {
                return Err(CoreError::EndOfInput);
            };
            match front.read_batch(ctx).await {
                Ok(res) => return Ok(res),
                Err(CoreError::EndOfInput) => {
                    let mut done = self.inputs.pop_front().unwrap();
                    let _ = done.close().await;
                    if let Some(next) = self.inputs.front_mut() {
                        next.connect(ctx).await?;
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
}

pub struct ReadUntilInput {
    child: Box<dyn Input>,
    check: Executor,
    done: bool,
}

impl ReadUntilInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let child_val = conf
            .config
            .get("input")
            .ok_or_else(|| CoreError::config("read_until requires an 'input' field"))?;
        let child_conf = ComponentConfig::from_json_value(child_val, "stdin", None)?;
        let child = ctx.registry.build_input(&child_conf, ctx)?;
        let check_str = conf
            .config
            .get("check")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("read_until requires a 'check' field"))?;
        let check = ctx.blobl_env.compile_mapping(check_str)?;
        Ok(Self {
            child,
            check,
            done: false,
        })
    }
}

#[async_trait]
impl Input for ReadUntilInput {
    fn input_type(&self) -> &'static str {
        "read_until"
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        self.child.connect(ctx).await
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        if self.done {
            return Err(CoreError::EndOfInput);
        }
        let (batch, ack) = self.child.read_batch(ctx).await?;
        let view = PartsView(&batch);
        for i in 0..batch.len() {
            if self.check.query_part(i, &view).unwrap_or(false) {
                self.done = true;
                break;
            }
        }
        Ok((batch, ack))
    }
}

pub struct ResourceInput {
    name: String,
}

#[async_trait]
impl Input for ResourceInput {
    fn input_type(&self) -> &'static str {
        "resource"
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        let res_input: Arc<AsyncMutex<Box<dyn Input>>> =
            ctx.resources.get_input(ctx.org_id(), &self.name)?;
        let mut guard = res_input.lock().await;
        guard.read_batch(ctx).await
    }
}

/// Implements `stdin` input (`internal/impl/io/input_stdin.go`): a reader thread scans stdin
/// (the `lines` scanner by default) and hands over each message as soon as it is read.
pub struct StdinInput {
    scanner: FileScannerKind,
    stream: Option<ScanStream>,
}

impl StdinInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        if ctx.file_root.is_some() {
            return Err(CoreError::config(
                "stdin input is not available to a stream confined to a file root",
            ));
        }
        let empty = serde_json::Map::new();
        let scanner = parse_scanner_config(conf.config.as_object().unwrap_or(&empty))?;
        Ok(Self {
            scanner,
            stream: None,
        })
    }
}

#[async_trait]
impl Input for StdinInput {
    fn input_type(&self) -> &'static str {
        "stdin"
    }

    async fn read_batch(
        &mut self,
        _ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        let scanner = &self.scanner;
        let stream = self.stream.get_or_insert_with(|| {
            let open = || {
                let annotate: Box<dyn Fn(&mut Part) + Send> = Box::new(|_: &mut Part| {});
                Ok((std::io::stdin(), annotate))
            };
            ScanStream::spawn(open, String::new(), scanner.clone(), 1, None)
        });
        match stream.recv().await {
            Ok(Some(batch)) => Ok((batch, AckResponder::noop())),
            Ok(None) => Err(CoreError::EndOfInput),
            Err(e) => Err(CoreError::other(format!("failed to read stdin: {e}"))),
        }
    }
}

pub struct EmptyInput;

#[async_trait]
impl Input for EmptyInput {
    fn input_type(&self) -> &'static str {
        "inproc"
    }

    async fn read_batch(
        &mut self,
        _ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        Err(CoreError::EndOfInput)
    }
}

#[cfg(test)]
mod scanner_config_tests {
    use super::*;

    fn scanner(conf: serde_json::Value) -> Result<FileScannerKind, CoreError> {
        parse_scanner_config(conf.as_object().expect("an object"))
    }

    #[test]
    fn an_unknown_scanner_is_refused_by_name() {
        let err =
            scanner(serde_json::json!({"scanner": {"regex_lines": {}}})).expect_err("refused");
        assert!(
            err.to_string()
                .contains("scanner type 'regex_lines' was not recognised"),
            "{err}"
        );
    }

    #[test]
    fn nested_scanners_read_a_compressed_stream_after_its_bom() {
        use std::io::Write;
        let conf = serde_json::json!({"scanner": {"skip_bom": {"into": {"decompress": {
            "algorithm": "gzip", "into": {"re_match": {"pattern": "(?m)^#"}}}}}}});
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"#a\n#b\n").expect("gz");
        let mut raw = vec![0xEF, 0xBB, 0xBF];
        raw.extend(gz.finish().expect("gz"));
        let mut parts = Vec::new();
        scan_reader(
            &mut raw.as_slice(),
            &scanner(conf).expect("scanner"),
            "",
            &mut |p| {
                parts.push(p);
                Ok(())
            },
        )
        .expect("parts");
        let bodies: Vec<Bytes> = parts.iter().map(Part::as_bytes_ref).collect();
        assert_eq!(
            bodies,
            [Bytes::from_static(b"#a\n"), Bytes::from_static(b"#b\n")]
        );
    }
}
