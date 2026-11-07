use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_bloblang::methods_strings::parse_xml_to_value;
use ekiben_bloblang::{FieldExpression, Value};
use ekiben_core::message::batch_collapsed_count;
use ekiben_core::{
    deserialize_bytes, serialize_bytes, Batch, ComponentConfig, CoreError, Part, PartsView,
    ProcContext, Processor, Registry,
};
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
use flate2::Compression;
use serde_json::Value as JsonValue;

/// Safety limits against zip/tar bombs (`Section 5 Memory & Security`).
pub const MAX_ARCHIVE_MEMBERS: usize = 10_000;
pub const MAX_ARCHIVE_UNPACKED_BYTES: u64 = 128 * 1024 * 1024; // 128 MiB

pub fn register(registry: &Registry) {
    registry.register_processor("archive", |conf, ctx| {
        Ok(Arc::new(ArchiveProcessor::from_config(conf, ctx)?))
    });

    registry.register_processor("unarchive", |conf, ctx| {
        // A member is a message the run holds, so the org's in-flight quota bounds how many a
        // json_array, json_documents, lines or csv input may split into past MAX_ARCHIVE_MEMBERS
        // (Bento sets no bound).
        let held = ctx.org.quotas.read().map_or(MAX_ARCHIVE_MEMBERS, |q| {
            q.max_in_flight_messages.max(MAX_ARCHIVE_MEMBERS)
        });
        Ok(Arc::new(
            UnarchiveProcessor::from_config(conf)?.with_max_members(held),
        ))
    });

    registry.register_processor("split", |conf, _ctx| {
        Ok(Arc::new(SplitProcessor::from_config(conf)?))
    });

    registry.register_processor("xml", |conf, _ctx| {
        Ok(Arc::new(XmlProcessor::from_config(conf)?))
    });

    registry.register_processor("compress", |conf, _ctx| {
        let algo = conf
            .config
            .get("algorithm")
            .and_then(JsonValue::as_str)
            .unwrap_or("gzip")
            .to_string();
        Ok(Arc::new(CompressProcessor { algorithm: algo }))
    });

    registry.register_processor("decompress", |conf, _ctx| {
        let algo = conf
            .config
            .get("algorithm")
            .and_then(JsonValue::as_str)
            .unwrap_or("gzip")
            .to_string();
        Ok(Arc::new(DecompressProcessor { algorithm: algo }))
    });
}

/// Implements `archive` (`internal/impl/pure/processor_archive.go`).
pub struct ArchiveProcessor {
    format: String,
    path: Option<FieldExpression>,
}

impl ArchiveProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("archive config must be an object"))?;
        let format = obj
            .get("format")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("archive requires a format field"))?
            .to_string();
        if !matches!(
            format.as_str(),
            "tar" | "zip" | "binary" | "lines" | "json_array" | "concatenate"
        ) {
            return Err(CoreError::config(format!(
                "archive format not recognised: {format}"
            )));
        }
        let path = match obj.get("path").and_then(JsonValue::as_str) {
            Some(s) if !s.is_empty() => Some(ctx.blobl_env.compile_field(s)?),
            _ => None,
        };
        Ok(Self { format, path })
    }
}

#[async_trait]
impl Processor for ArchiveProcessor {
    fn processor_type(&self) -> &'static str {
        "archive"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let total_collapsed = batch_collapsed_count(&batch);
        let mut first_part = batch[0].shallow_copy();

        match self.format.as_str() {
            "json_array" => {
                let mut arr = Vec::with_capacity(batch.len());
                for (i, part) in batch.iter_mut().enumerate() {
                    match part.as_structured() {
                        Ok(doc) => arr.push(doc),
                        Err(e) => {
                            ctx.on_error(part, format!("failed to parse message as JSON: {e}"));
                            return Ok(vec![batch]);
                        }
                    }
                    let _ = i;
                }
                first_part.set_structured(Value::Array(arr.into()));
            }
            "lines" => {
                let mut joined = Vec::new();
                for (i, part) in batch.iter_mut().enumerate() {
                    if i > 0 {
                        joined.push(b'\n');
                    }
                    joined.extend_from_slice(&part.as_bytes());
                }
                first_part.set_bytes(Bytes::from(joined));
            }
            "concatenate" => {
                let mut joined = Vec::new();
                for part in &mut batch {
                    joined.extend_from_slice(&part.as_bytes());
                }
                first_part.set_bytes(Bytes::from(joined));
            }
            "binary" => {
                let raw_vecs: Vec<Bytes> = batch.iter_mut().map(Part::as_bytes).collect();
                let slices: Vec<&[u8]> = raw_vecs.iter().map(Bytes::as_ref).collect();
                first_part.set_bytes(Bytes::from(serialize_bytes(&slices)));
            }
            "tar" => {
                for p in &mut batch {
                    let _ = p.as_bytes();
                }
                let view = PartsView(&batch);
                let mut buf = Vec::new();
                {
                    let mut tw = tar::Builder::new(&mut buf);
                    for (i, part) in batch.iter().enumerate() {
                        let name = match &self.path {
                            Some(f) => f
                                .resolve_string(i, &view)
                                .unwrap_or_else(|_| format!("part_{i}")),
                            None => format!("part_{i}"),
                        };
                        let data = part.as_bytes_ref();
                        let mut header = tar::Header::new_gnu();
                        header.set_size(data.len() as u64);
                        header.set_mode(0o666);
                        header.set_cksum();
                        tw.append_data(&mut header, name, &data[..])
                            .map_err(|e| CoreError::other(e.to_string()))?;
                    }
                    tw.finish().map_err(|e| CoreError::other(e.to_string()))?;
                }
                first_part.set_bytes(Bytes::from(buf));
            }
            "zip" => {
                for p in &mut batch {
                    let _ = p.as_bytes();
                }
                let view = PartsView(&batch);
                let mut buf = Cursor::new(Vec::new());
                {
                    let mut zw = zip::ZipWriter::new(&mut buf);
                    let opts = zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated);
                    for (i, part) in batch.iter().enumerate() {
                        let name = match &self.path {
                            Some(f) => f
                                .resolve_string(i, &view)
                                .unwrap_or_else(|_| format!("part_{i}")),
                            None => format!("part_{i}"),
                        };
                        zw.start_file(name, opts)
                            .map_err(|e| CoreError::other(e.to_string()))?;
                        zw.write_all(&part.as_bytes_ref())
                            .map_err(|e| CoreError::other(e.to_string()))?;
                    }
                    zw.finish().map_err(|e| CoreError::other(e.to_string()))?;
                }
                first_part.set_bytes(Bytes::from(buf.into_inner()));
            }
            _ => unreachable!(),
        }

        first_part.add_collapsed_count(total_collapsed);
        Ok(vec![vec![first_part]])
    }
}

/// Implements `unarchive` (`internal/impl/pure/processor_unarchive.go`).
/// Streams members out one by one with strict zip-bomb caps (`MAX_ARCHIVE_MEMBERS`, `MAX_ARCHIVE_UNPACKED_BYTES`).
/// ponytail: unarchive materializes the resulting parts into the output `Batch` `Vec<Part>` because Bento's
/// `Processor` contract operates on a `Batch`; to bound peak memory, `MAX_ARCHIVE_MEMBERS` and `MAX_ARCHIVE_UNPACKED_BYTES`
/// cap each unarchived part, and file inputs use streaming scanners directly.
pub struct UnarchiveProcessor {
    format: String,
    csv_delim: u8,
    max_members: usize,
}

impl UnarchiveProcessor {
    /// Bounds the members a non-compressed format splits into (default [`MAX_ARCHIVE_MEMBERS`]).
    pub fn with_max_members(mut self, max_members: usize) -> Self {
        self.max_members = max_members;
        self
    }

    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let format = conf
            .config
            .get("format")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("unarchive requires a format field"))?
            .to_string();

        if let Some(after) = format.strip_prefix("csv:") {
            let mut chars = after.chars();
            let ch = chars
                .next()
                .ok_or_else(|| CoreError::config("csv format requires a non-empty delimiter"))?;
            if chars.next().is_some() || !ch.is_ascii() {
                return Err(CoreError::config(
                    "csv format requires a single character delimiter",
                ));
            }
            return Ok(Self {
                format: "csv".to_string(),
                csv_delim: ch as u8,
                max_members: MAX_ARCHIVE_MEMBERS,
            });
        }

        if !matches!(
            format.as_str(),
            "tar"
                | "zip"
                | "binary"
                | "lines"
                | "json_documents"
                | "xml_documents_to_json"
                | "xml_documents_to_json:cast"
                | "json_array"
                | "json_map"
                | "csv"
        ) {
            return Err(CoreError::config(format!(
                "archive format not recognised: {format}"
            )));
        }

        Ok(Self {
            format,
            csv_delim: b',',
            max_members: MAX_ARCHIVE_MEMBERS,
        })
    }

    fn unarchive_part(&self, part: &mut Part) -> Result<Batch, String> {
        match self.format.as_str() {
            "json_array" => {
                let doc = part
                    .as_structured()
                    .map_err(|e| format!("failed to parse message into JSON array: {e}"))?;
                let Value::Array(arr) = doc else {
                    return Err(format!(
                        "failed to parse message into JSON array: invalid type '{}'",
                        doc.type_name()
                    ));
                };
                if arr.len() > self.max_members {
                    return Err(format!(
                        "json_array member count {} exceeds safety limit ({})",
                        arr.len(),
                        self.max_members
                    ));
                }
                let mut out = Vec::with_capacity(arr.len());
                for item in arr {
                    let mut np = part.shallow_copy();
                    np.set_structured(item);
                    out.push(np);
                }
                Ok(out)
            }
            "json_map" => {
                let doc = part
                    .as_structured()
                    .map_err(|e| format!("failed to parse message into JSON map: {e}"))?;
                let Value::Object(map) = doc else {
                    return Err(format!(
                        "failed to parse message into JSON map: invalid type '{}'",
                        doc.type_name()
                    ));
                };
                let mut out = Vec::with_capacity(map.len());
                for (k, v) in map {
                    let mut np = part.shallow_copy();
                    np.set_structured(v);
                    np.meta_set_str("archive_key", k);
                    out.push(np);
                }
                Ok(out)
            }
            "json_documents" => {
                let bytes = part.as_bytes();
                let stream =
                    serde_json::Deserializer::from_slice(&bytes).into_iter::<serde_json::Value>();
                let mut out = Vec::new();
                for item_res in stream {
                    let jv = item_res.map_err(|e| e.to_string())?;
                    if out.len() >= self.max_members {
                        return Err("json_documents member count exceeds safety limit".to_string());
                    }
                    let mut np = part.shallow_copy();
                    np.set_structured(Value::from_json_value(jv));
                    out.push(np);
                }
                Ok(out)
            }
            "lines" => {
                let bytes = part.as_bytes();
                let mut out = Vec::new();
                for line in bytes.split(|&b| b == b'\n') {
                    if out.len() >= self.max_members {
                        return Err("lines count exceeds safety limit".to_string());
                    }
                    let mut np = part.shallow_copy();
                    np.set_bytes(Bytes::copy_from_slice(line));
                    out.push(np);
                }
                Ok(out)
            }
            "binary" => {
                let bytes = part.as_bytes();
                let parts = deserialize_bytes(&bytes).map_err(|e| e.to_string())?;
                let mut out = Vec::with_capacity(parts.len());
                for p in parts {
                    let mut np = part.shallow_copy();
                    np.set_bytes(Bytes::from(p));
                    out.push(np);
                }
                Ok(out)
            }
            "tar" => {
                let bytes = part.as_bytes();
                let mut archive = tar::Archive::new(Cursor::new(bytes.as_ref()));
                let entries = archive.entries().map_err(|e| e.to_string())?;
                let mut out = Vec::new();
                let mut total_unpacked = 0u64;

                for entry_res in entries {
                    if out.len() >= MAX_ARCHIVE_MEMBERS {
                        return Err(format!(
                            "tar archive member count exceeds maximum allowed ({MAX_ARCHIVE_MEMBERS})"
                        ));
                    }
                    let entry = entry_res.map_err(|e| e.to_string())?;
                    let filename = entry
                        .path()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let remaining_budget =
                        MAX_ARCHIVE_UNPACKED_BYTES.saturating_sub(total_unpacked);
                    let mut buf = Vec::new();
                    entry
                        .take(remaining_budget + 1)
                        .read_to_end(&mut buf)
                        .map_err(|e| e.to_string())?;
                    total_unpacked += buf.len() as u64;
                    if total_unpacked > MAX_ARCHIVE_UNPACKED_BYTES {
                        return Err(format!(
                            "tar archive unpacked size exceeds safety limit ({MAX_ARCHIVE_UNPACKED_BYTES} bytes)"
                        ));
                    }
                    let mut np = part.shallow_copy();
                    np.set_bytes(Bytes::from(buf));
                    np.meta_set_str("archive_filename", filename);
                    out.push(np);
                }
                Ok(out)
            }
            "zip" => {
                let bytes = part.as_bytes();
                let mut archive =
                    zip::ZipArchive::new(Cursor::new(bytes.as_ref())).map_err(|e| e.to_string())?;
                if archive.len() > MAX_ARCHIVE_MEMBERS {
                    return Err(format!(
                        "zip archive member count ({}) exceeds safety limit ({MAX_ARCHIVE_MEMBERS})",
                        archive.len()
                    ));
                }
                let mut out = Vec::with_capacity(archive.len());
                let mut total_unpacked = 0u64;

                for i in 0..archive.len() {
                    let file = archive.by_index(i).map_err(|e| e.to_string())?;
                    let filename = file.name().map_err(|e| e.to_string())?.into_owned();
                    let remaining_budget =
                        MAX_ARCHIVE_UNPACKED_BYTES.saturating_sub(total_unpacked);
                    let mut buf = Vec::new();
                    file.take(remaining_budget + 1)
                        .read_to_end(&mut buf)
                        .map_err(|e| e.to_string())?;
                    total_unpacked += buf.len() as u64;
                    if total_unpacked > MAX_ARCHIVE_UNPACKED_BYTES {
                        return Err(format!(
                            "zip bomb detected: unpacked size exceeds safety limit ({MAX_ARCHIVE_UNPACKED_BYTES} bytes)"
                        ));
                    }
                    let mut np = part.shallow_copy();
                    np.set_bytes(Bytes::from(buf));
                    np.meta_set_str("archive_filename", filename);
                    out.push(np);
                }
                Ok(out)
            }
            "csv" => {
                let bytes = part.as_bytes();
                let mut rdr = csv::ReaderBuilder::new()
                    .has_headers(false)
                    .delimiter(self.csv_delim)
                    .from_reader(bytes.as_ref());
                let mut headers: Option<Vec<String>> = None;
                let mut out = Vec::new();

                for rec_res in rdr.records() {
                    let rec =
                        rec_res.map_err(|e| format!("failed to parse message as csv: {e}"))?;
                    let Some(ref hdrs) = headers else {
                        headers = Some(rec.iter().map(String::from).collect());
                        continue;
                    };
                    if rec.len() < hdrs.len() {
                        return Err(
                            "failed to parse message as csv: row has too few values".to_string()
                        );
                    }
                    if rec.len() > hdrs.len() {
                        return Err(
                            "failed to parse message as csv: row has too many values".to_string()
                        );
                    }
                    if out.len() >= self.max_members {
                        return Err("csv row count exceeds safety limit".to_string());
                    }
                    let mut obj = BTreeMap::new();
                    for (h, val) in hdrs.iter().zip(rec.iter()) {
                        obj.insert(h.clone(), Value::String(val.to_string()));
                    }
                    let mut np = part.shallow_copy();
                    np.set_structured(Value::Object(obj.into()));
                    out.push(np);
                }
                Ok(out)
            }
            "xml_documents_to_json" | "xml_documents_to_json:cast" => {
                let cast = self.format.ends_with(":cast");
                let bytes = part.as_bytes();
                let val = parse_xml_to_value(&bytes, cast)?;
                let mut np = part.shallow_copy();
                np.set_structured(val);
                Ok(vec![np])
            }
            _ => unreachable!(),
        }
    }
}

#[async_trait]
impl Processor for UnarchiveProcessor {
    fn processor_type(&self) -> &'static str {
        "unarchive"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let mut out = Vec::new();
        for mut part in batch {
            match self.unarchive_part(&mut part) {
                Ok(expanded) => out.extend(expanded),
                Err(err) => {
                    ctx.on_error(&mut part, err);
                    out.push(part);
                }
            }
        }
        if out.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![out])
        }
    }
}

/// Implements `split` (`internal/impl/pure/processor_split.go`).
pub struct SplitProcessor {
    size: usize,
    byte_size: usize,
}

impl SplitProcessor {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let size = conf
            .config
            .get("size")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1) as usize;
        let byte_size = conf
            .config
            .get("byte_size")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as usize;
        Ok(Self { size, byte_size })
    }
}

#[async_trait]
impl Processor for SplitProcessor {
    fn processor_type(&self) -> &'static str {
        "split"
    }

    async fn process_batch(
        &self,
        _ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let mut batches = Vec::new();
        let mut next_batch = Vec::new();
        let mut current_bytes = 0usize;

        for mut part in batch {
            let part_len = if self.byte_size > 0 {
                part.as_bytes().len()
            } else {
                0
            };
            if ((self.size > 0 && next_batch.len() >= self.size)
                || (self.byte_size > 0 && current_bytes + part_len > self.byte_size))
                && !next_batch.is_empty()
            {
                batches.push(std::mem::take(&mut next_batch));
                current_bytes = 0;
            }
            next_batch.push(part);
            current_bytes += part_len;
        }
        if !next_batch.is_empty() {
            batches.push(next_batch);
        }
        Ok(batches)
    }
}

/// Implements `xml` processor (`internal/impl/xml/processor.go`).
pub struct XmlProcessor {
    cast: bool,
}

impl XmlProcessor {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let operator = conf
            .config
            .get("operator")
            .and_then(JsonValue::as_str)
            .unwrap_or("to_json");
        if operator != "to_json" {
            return Err(CoreError::config(format!(
                "xml operator not recognised: {operator}"
            )));
        }
        let cast = conf
            .config
            .get("cast")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        Ok(Self { cast })
    }
}

#[async_trait]
impl Processor for XmlProcessor {
    fn processor_type(&self) -> &'static str {
        "xml"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        for part in &mut batch {
            let bytes = part.as_bytes();
            match parse_xml_to_value(&bytes, self.cast) {
                Ok(val) => {
                    part.set_structured(val);
                }
                Err(e) => {
                    ctx.on_error(part, e);
                }
            }
        }
        Ok(vec![batch])
    }
}

pub struct CompressProcessor {
    algorithm: String,
}

#[async_trait]
impl Processor for CompressProcessor {
    fn processor_type(&self) -> &'static str {
        "compress"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        for part in &mut batch {
            let bytes = part.as_bytes();
            let res: Result<Vec<u8>, String> = match self.algorithm.as_str() {
                "gzip" | "pgzip" => {
                    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
                    enc.write_all(&bytes)
                        .and_then(|_| enc.finish())
                        .map_err(|e| e.to_string())
                }
                "zlib" => {
                    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
                    enc.write_all(&bytes)
                        .and_then(|_| enc.finish())
                        .map_err(|e| e.to_string())
                }
                "flate" => {
                    let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
                    enc.write_all(&bytes)
                        .and_then(|_| enc.finish())
                        .map_err(|e| e.to_string())
                }
                other => Err(format!("unsupported compression algorithm: {other}")),
            };
            match res {
                Ok(compressed) => {
                    part.set_bytes(Bytes::from(compressed));
                }
                Err(e) => ctx.on_error(part, e),
            }
        }
        Ok(vec![batch])
    }
}

pub struct DecompressProcessor {
    algorithm: String,
}

#[async_trait]
impl Processor for DecompressProcessor {
    fn processor_type(&self) -> &'static str {
        "decompress"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        for part in &mut batch {
            let bytes = part.as_bytes();
            let mut out = Vec::new();
            let res: Result<(), String> = match self.algorithm.as_str() {
                "gzip" | "pgzip" => GzDecoder::new(bytes.as_ref())
                    .take(MAX_ARCHIVE_UNPACKED_BYTES + 1)
                    .read_to_end(&mut out)
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
                "zlib" => ZlibDecoder::new(bytes.as_ref())
                    .take(MAX_ARCHIVE_UNPACKED_BYTES + 1)
                    .read_to_end(&mut out)
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
                "flate" => DeflateDecoder::new(bytes.as_ref())
                    .take(MAX_ARCHIVE_UNPACKED_BYTES + 1)
                    .read_to_end(&mut out)
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
                other => Err(format!("unsupported decompression algorithm: {other}")),
            };
            match res {
                Ok(()) if (out.len() as u64) <= MAX_ARCHIVE_UNPACKED_BYTES => {
                    part.set_bytes(Bytes::from(out));
                }
                Ok(()) => ctx.on_error(
                    part,
                    format!("decompressed data exceeds safety limit ({MAX_ARCHIVE_UNPACKED_BYTES} bytes)"),
                ),
                Err(e) => ctx.on_error(part, e),
            }
        }
        Ok(vec![batch])
    }
}
