use std::collections::{BTreeMap, VecDeque};
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
    interval: Option<Duration>,
    remaining: Option<usize>,
    batch_size: usize,
    first_run: bool,
    auto_replay_nacks: bool,
    replay_queue: VecDeque<Batch>,
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

        let interval = match obj.get("interval").and_then(JsonValue::as_str) {
            Some(s) if !s.trim().is_empty() && s.trim() != "0" && s.trim() != "0s" => {
                // In `render.rs`, `interval` can be a duration like `"60s"` or `"100ms"` or a cron string;
                // if it's a duration string, parse it; if it's a cron expression, default interval to 60s in resident mode.
                parse_duration_str(s).ok().or(Some(Duration::from_secs(60)))
            }
            _ => None,
        };

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
            interval,
            remaining,
            batch_size,
            first_run: true,
            auto_replay_nacks,
            replay_queue: VecDeque::new(),
        })
    }
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
        if let Some(replayed) = self.replay_queue.pop_front() {
            return Ok((replayed, AckResponder::noop()));
        }

        if let Some(rem) = self.remaining {
            if rem == 0 {
                return Err(CoreError::EndOfInput);
            }
        }

        if !self.first_run {
            if let Some(iv) = self.interval {
                let mut rx = ctx.shutdown_rx.clone();
                tokio::select! {
                    _ = tokio::time::sleep(iv) => {}
                    _ = rx.changed() => {
                        if *rx.borrow() {
                            return Err(CoreError::EndOfInput);
                        }
                    }
                }
            }
        }
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

        let _ = self.auto_replay_nacks;
        Ok((batch, AckResponder::noop()))
    }
}

#[derive(Clone, Debug)]
enum FileScannerKind {
    Lines,
    ToTheEnd,
    JsonDocuments,
    Csv {
        parse_header_row: bool,
        delimiter: u8,
        lazy_quotes: bool,
    },
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

/// Implements `file` and `csv` inputs (`internal/impl/io/input_file.go`).
pub struct FileInput {
    paths: VecDeque<PathBuf>,
    scanner: FileScannerKind,
    delete_on_finish: bool,
    batch_count: usize,
    pending_batches: VecDeque<(Batch, Option<PathBuf>)>,
}

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
            pending_batches: VecDeque::new(),
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
        let parse_header_row = obj
            .get("parse_header_row")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true);
        let delimiter = obj
            .get("delimiter")
            .and_then(JsonValue::as_str)
            .and_then(|s| s.bytes().next())
            .unwrap_or(b',');
        let lazy_quotes = obj
            .get("lazy_quotes")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
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
            scanner: FileScannerKind::Csv {
                parse_header_row,
                delimiter,
                lazy_quotes,
            },
            delete_on_finish,
            batch_count,
            pending_batches: VecDeque::new(),
        })
    }

    async fn load_next_file(&mut self) -> Result<bool, CoreError> {
        let Some(next_path) = self.paths.pop_front() else {
            return Ok(false);
        };
        let path_for_read = next_path.clone();
        let scanner = self.scanner.clone();
        let batch_count = self.batch_count;

        let batches = tokio::task::spawn_blocking(move || -> Result<Vec<Batch>, CoreError> {
            let metadata = std::fs::metadata(&path_for_read).map_err(|e| {
                CoreError::other(format!("failed to stat '{}': {e}", path_for_read.display()))
            })?;
            let mod_time: DateTime<Utc> = metadata
                .modified()
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(|_| Utc::now());
            let mod_unix = mod_time.timestamp();
            let mod_rfc3339 = mod_time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let path_str = path_for_read.to_string_lossy().into_owned();

            let raw = std::fs::read(&path_for_read).map_err(|e| {
                CoreError::other(format!("failed to read '{}': {e}", path_for_read.display()))
            })?;

            let annotate = |mut p: Part| -> Part {
                p.meta_set_str("path", path_str.clone());
                p.meta_set("mod_time_unix", Value::Int(mod_unix));
                p.meta_set_str("mod_time", mod_rfc3339.clone());
                p
            };

            let parts: Vec<Part> = scan_parts(raw, scanner)?
                .into_iter()
                .map(annotate)
                .collect();

            let mut out_batches = Vec::new();
            for chunk in parts.chunks(batch_count.max(1)) {
                out_batches.push(chunk.to_vec());
            }
            Ok(out_batches)
        })
        .await
        .map_err(|e| CoreError::other(e.to_string()))??;

        let total = batches.len();
        for (i, b) in batches.into_iter().enumerate() {
            let delete_target = if self.delete_on_finish && i + 1 == total {
                Some(next_path.clone())
            } else {
                None
            };
            self.pending_batches.push_back((b, delete_target));
        }

        Ok(true)
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
            if let Some((batch, delete_path)) = self.pending_batches.pop_front() {
                let ack = match delete_path {
                    Some(p) => AckResponder::from_fn(move |err| {
                        if err.is_none() {
                            let _ = std::fs::remove_file(p);
                        }
                    }),
                    None => AckResponder::noop(),
                };
                return Ok((batch, ack));
            }
            if !self.load_next_file().await? {
                return Err(CoreError::EndOfInput);
            }
        }
    }
}

/// Splits raw bytes into message parts with a file/stdin scanner.
fn scan_parts(mut raw: Vec<u8>, scanner: FileScannerKind) -> Result<Vec<Part>, CoreError> {
    let mut parts: Vec<Part> = Vec::new();
    match scanner {
        FileScannerKind::Decompress { algorithm, into } => {
            return scan_parts(algorithm.decompress(&raw)?, *into);
        }
        FileScannerKind::SkipBom { into } => {
            let bom = raw.len() - crate::scanner_skip_bom::skip_bom(&raw).len();
            raw.drain(..bom);
            return scan_parts(raw, *into);
        }
        FileScannerKind::ReMatch(re) => {
            parts.extend(re.split(&raw)?.into_iter().map(Part::new));
        }
        FileScannerKind::XmlDocuments(xml) => {
            parts.extend(xml.documents(&raw)?.into_iter().map(Part::from_structured));
        }
        FileScannerKind::ToTheEnd => {
            parts.push(Part::new(Bytes::from(raw)));
        }
        FileScannerKind::Lines => {
            let s = String::from_utf8_lossy(&raw);
            for line in s.lines() {
                parts.push(Part::from_str(line));
            }
        }
        FileScannerKind::JsonDocuments => {
            let stream =
                serde_json::Deserializer::from_slice(&raw).into_iter::<serde_json::Value>();
            for doc_res in stream {
                let jv = doc_res.map_err(|e| CoreError::other(e.to_string()))?;
                parts.push(Part::from_json(jv));
            }
        }
        FileScannerKind::Csv {
            parse_header_row,
            delimiter,
            lazy_quotes,
        } => {
            let mut rdr = csv::ReaderBuilder::new()
                .has_headers(false)
                .delimiter(delimiter)
                .flexible(lazy_quotes)
                .from_reader(raw.as_slice());
            let mut headers: Option<Vec<String>> = None;
            for rec_res in rdr.records() {
                let rec = rec_res.map_err(|e| CoreError::other(e.to_string()))?;
                if parse_header_row && headers.is_none() {
                    headers = Some(rec.iter().map(String::from).collect());
                    continue;
                }
                if let Some(ref hdrs) = headers {
                    let mut obj = BTreeMap::new();
                    for (h, v) in hdrs.iter().zip(rec.iter()) {
                        obj.insert(h.clone(), Value::String(v.to_string()));
                    }
                    parts.push(Part::from_structured(Value::Object(obj.into())));
                } else {
                    let arr: Vec<Value> =
                        rec.iter().map(|v| Value::String(v.to_string())).collect();
                    parts.push(Part::from_structured(Value::Array(arr.into())));
                }
            }
        }
        FileScannerKind::Tar => {
            let mut arch = tar::Archive::new(Cursor::new(raw.as_slice()));
            for entry_res in arch
                .entries()
                .map_err(|e| CoreError::other(e.to_string()))?
            {
                let mut entry = entry_res.map_err(|e| CoreError::other(e.to_string()))?;
                let mut buf = Vec::new();
                use std::io::Read;
                entry
                    .read_to_end(&mut buf)
                    .map_err(|e| CoreError::other(e.to_string()))?;
                parts.push(Part::new(Bytes::from(buf)));
            }
        }
        FileScannerKind::Chunker { size } => {
            for chunk in raw.chunks(size.max(1)) {
                parts.push(Part::new(Bytes::copy_from_slice(chunk)));
            }
        }
    }
    Ok(parts)
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
            "csv" => Ok(FileScannerKind::Csv {
                parse_header_row: true,
                delimiter: b',',
                lazy_quotes: false,
            }),
            "tar" => Ok(FileScannerKind::Tar),
            _ => Ok(FileScannerKind::Lines),
        };
    }
    match obj.get("scanner").and_then(JsonValue::as_object) {
        Some(scanner_obj) => parse_scanner(scanner_obj),
        None => Ok(FileScannerKind::Lines),
    }
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
        let name = conf.get("algorithm").and_then(JsonValue::as_str).ok_or_else(|| {
            CoreError::config("decompress: field 'algorithm' is required")
        })?;
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
    if let Some(csv_val) = scanner_obj.get("csv") {
        let parse_header_row = csv_val
            .get("parse_header_row")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true);
        let delimiter = csv_val
            .get("custom_delimiter")
            .and_then(JsonValue::as_str)
            .and_then(|s| s.bytes().next())
            .unwrap_or(b',');
        let lazy_quotes = csv_val
            .get("lazy_quotes")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        return Ok(FileScannerKind::Csv {
            parse_header_row,
            delimiter,
            lazy_quotes,
        });
    }
    Ok(FileScannerKind::Lines)
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

/// Implements `stdin` input (`internal/impl/io/input_stdin.go`): the `lines` scanner (default) streams one
/// message per line; any other scanner reads stdin to its end first.
const STDIN_BUFFER: usize = 1 << 20;

pub struct StdinInput {
    scanner: FileScannerKind,
    lines: Option<tokio::io::Lines<tokio::io::BufReader<tokio::io::Stdin>>>,
    pending: VecDeque<Part>,
    done: bool,
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
            lines: None,
            pending: VecDeque::new(),
            done: false,
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
        use tokio::io::{AsyncBufReadExt, AsyncReadExt};
        if let Some(p) = self.pending.pop_front() {
            return Ok((vec![p], AckResponder::noop()));
        }
        if self.done {
            return Err(CoreError::EndOfInput);
        }
        if matches!(self.scanner, FileScannerKind::Lines) {
            let lines = self.lines.get_or_insert_with(|| {
                // tokio reads stdin on the blocking pool, one hop per buffer fill: a large
                // buffer keeps the hops rare (8 KiB capped a stream at ~60k lines/s).
                tokio::io::BufReader::with_capacity(STDIN_BUFFER, tokio::io::stdin()).lines()
            });
            return match lines.next_line().await {
                Ok(Some(line)) => Ok((vec![Part::from_str(&line)], AckResponder::noop())),
                Ok(None) => Err(CoreError::EndOfInput),
                Err(e) => Err(CoreError::other(format!("failed to read stdin: {e}"))),
            };
        }
        let mut raw = Vec::new();
        tokio::io::stdin()
            .read_to_end(&mut raw)
            .await
            .map_err(|e| CoreError::other(format!("failed to read stdin: {e}")))?;
        self.done = true;
        self.pending = scan_parts(raw, self.scanner.clone())?.into();
        match self.pending.pop_front() {
            Some(p) => Ok((vec![p], AckResponder::noop())),
            None => Err(CoreError::EndOfInput),
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
