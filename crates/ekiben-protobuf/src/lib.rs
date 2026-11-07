#![forbid(unsafe_code)]
//! `ekiben-protobuf` — the `protobuf` processor (`internal/impl/protobuf/processor_protobuf.go`, Bento 1.22.0).
//!
//! Bento parses every `.proto` file under `import_paths` at start (jhump protoparse) and converts each message with
//! `google.golang.org/protobuf` v1.36.12: `proto.Unmarshal` + `protojson.Marshal` for `to_json`,
//! `protojson.Unmarshal` + `proto.Marshal` for `from_json`. Here protox compiles the files, prost-reflect decodes
//! and encodes, and [`to_json`] writes protojson's exact text: fields in declaration order, JSON names (or proto
//! names), 64-bit integers and bytes as strings, enum names, Go's float format, the well-known types. Bento 1.22.0's
//! binary adds no random space after commas (protojson `detrand`), so neither does this.
//!
//! The Buf Schema Registry (`bsr`) needs network access at start and no pipeline uses it: a config naming it is
//! refused at construction with that reason.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use ekiben_core::{Batch, ComponentConfig, CoreError, ProcContext, Processor, Registry};
use prost::Message as _;
use prost_reflect::{
    Cardinality, DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MapKey, MessageDescriptor,
    ReflectMessage, Value as PValue,
};
use protox::file::{ChainFileResolver, File, FileResolver, GoogleFileResolver};
use serde_json::Value as JsonValue;

/// Upper bound on the bytes of `.proto` sources read from the import paths.
pub const MAX_SCHEMA_BYTES: u64 = 16 * 1024 * 1024;
/// Message nesting allowed while writing JSON (the Go decoder stops at 10 000; prost at 100).
const MAX_DEPTH: usize = 100;

pub fn register(registry: &Registry) {
    registry.register_processor("protobuf", |conf, ctx| {
        Ok(Arc::new(ProtobufProcessor::from_config_in(
            conf,
            ctx.file_root.as_deref(),
        )?))
    });
}

#[derive(Clone, Copy)]
struct JsonOptions {
    use_proto_names: bool,
    emit_unpopulated: bool,
}

enum Operator {
    ToJson(MessageDescriptor, JsonOptions),
    FromJson(MessageDescriptor, bool),
}

/// Implements `protobuf`.
pub struct ProtobufProcessor {
    op: Operator,
    message: String,
}

impl ProtobufProcessor {
    /// Builds the processor with import paths read as given (Bento's behaviour, for a trusted config).
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        Self::from_config_in(conf, None)
    }

    /// Builds the processor with every import path, and every `.proto` file it holds, confined to `root` when set
    /// (the stream's file root): no `..`, no symlink out of it.
    pub fn from_config_in(conf: &ComponentConfig, root: Option<&Path>) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("protobuf config must be an object"))?;
        for k in obj.keys() {
            if !matches!(
                k.as_str(),
                "operator"
                    | "message"
                    | "import_paths"
                    | "discard_unknown"
                    | "use_proto_names"
                    | "emit_unpopulated"
                    | "bsr"
            ) {
                return Err(CoreError::config(format!(
                    "protobuf: field {k} not recognised"
                )));
            }
        }
        let bool_field = |name: &str| -> Result<bool, CoreError> {
            match obj.get(name) {
                None => Ok(false),
                Some(JsonValue::Bool(b)) => Ok(*b),
                Some(_) => Err(CoreError::config(format!(
                    "protobuf field {name}: expected bool value"
                ))),
            }
        };
        let operator = obj
            .get("operator")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("protobuf: field operator is required"))?;
        let message = obj
            .get("message")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("protobuf: field message is required"))?
            .to_string();
        let import_paths: Vec<String> = match obj.get("import_paths") {
            None => Vec::new(),
            Some(JsonValue::Array(a)) => a
                .iter()
                .map(|v| {
                    v.as_str().map(str::to_string).ok_or_else(|| {
                        CoreError::config("protobuf field import_paths: expected strings")
                    })
                })
                .collect::<Result<_, _>>()?,
            Some(_) => {
                return Err(CoreError::config(
                    "protobuf field import_paths: expected array",
                ))
            }
        };
        let bsr_set = obj
            .get("bsr")
            .and_then(JsonValue::as_array)
            .is_some_and(|a| !a.is_empty());
        if obj.get("import_paths").is_none() && !bsr_set {
            return Err(CoreError::config(
                "at least one of `import_paths`and `bsr` must be set",
            ));
        }
        let import_paths = import_paths
            .iter()
            .map(|p| ekiben_io_file::path_guard::sanitize_file_path(p, root))
            .collect::<Result<Vec<PathBuf>, CoreError>>()?;
        let shown: Vec<String> = obj
            .get("import_paths")
            .and_then(JsonValue::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if bsr_set {
            if !import_paths.is_empty() {
                return Err(CoreError::config(
                    "both `import_paths` and `bsr` can't be set simultaneously",
                ));
            }
            return Err(CoreError::config(
                "protobuf: bsr (Buf Schema Registry) is not supported by ekiben; vendor the .proto files and use import_paths",
            ));
        }
        let opts = JsonOptions {
            use_proto_names: bool_field("use_proto_names")?,
            emit_unpopulated: bool_field("emit_unpopulated")?,
        };
        let discard_unknown = bool_field("discard_unknown")?;
        let op = match operator {
            "to_json" | "from_json" if message.is_empty() => {
                return Err(CoreError::config("message field must not be empty"))
            }
            "to_json" => {
                let pool = load_descriptors(&import_paths, root).map_err(CoreError::config)?;
                let md = pool.get_message_by_name(&message).ok_or_else(|| {
                    CoreError::config(format!(
                        "unable to find message '{message}' definition within '{}'",
                        go_list(&shown)
                    ))
                })?;
                Operator::ToJson(md, opts)
            }
            "from_json" => {
                let pool = load_descriptors(&import_paths, root).map_err(CoreError::config)?;
                // Bento's type registry holds top-level messages and their direct children only.
                let md = pool
                    .get_message_by_name(&message)
                    .filter(|m| {
                        m.parent_message()
                            .is_none_or(|p| p.parent_message().is_none())
                    })
                    .ok_or_else(|| {
                        CoreError::config(format!(
                            "unable to find message '{message}' definition within '{}'",
                            go_list(&shown)
                        ))
                    })?;
                Operator::FromJson(md, discard_unknown)
            }
            other => {
                return Err(CoreError::config(format!(
                    "protobuf operator not recognised: {other}"
                )))
            }
        };
        Ok(Self { op, message })
    }

    /// Converts one message body; the error text is Bento's.
    pub fn convert(&self, input: &[u8]) -> Result<Vec<u8>, String> {
        let msg = &self.message;
        match &self.op {
            Operator::ToJson(md, opts) => {
                let dynm = DynamicMessage::decode(md.clone(), input).map_err(|_| {
                    format!("failed to unmarshal protobuf message '{msg}': proto: cannot parse invalid wire-format data")
                })?;
                check_required(&dynm).map_err(|f| {
                    format!("failed to unmarshal protobuf message '{msg}': proto: required field {f} not set")
                })?;
                let mut out = Vec::with_capacity(input.len() * 2);
                write_message(&mut out, &dynm, *opts, 0).map_err(|e| {
                    format!("failed to unmarshal JSON protobuf message '{msg}': {e}")
                })?;
                Ok(out)
            }
            Operator::FromJson(md, discard_unknown) => {
                let mut de = serde_json::Deserializer::from_slice(input);
                let options =
                    prost_reflect::DeserializeOptions::new().deny_unknown_fields(!discard_unknown);
                let dynm = DynamicMessage::deserialize_with_options(md.clone(), &mut de, &options)
                    .and_then(|m| de.end().map(|()| m))
                    .map_err(|e| format!("failed to unmarshal JSON message '{msg}': {e}"))?;
                check_required(&dynm).map_err(|f| {
                    format!("failed to marshal protobuf message '{msg}': proto: required field {f} not set")
                })?;
                Ok(dynm.encode_to_vec())
            }
        }
    }
}

#[async_trait]
impl Processor for ProtobufProcessor {
    fn processor_type(&self) -> &'static str {
        "protobuf"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        for part in batch.iter_mut() {
            match self.convert(&part.as_bytes()) {
                Ok(out) => {
                    part.set_bytes(out);
                }
                Err(e) => ctx.on_error(part, e),
            }
        }
        Ok(vec![batch])
    }
}

/// Go's `%v` of a string slice.
fn go_list(paths: &[String]) -> String {
    format!("[{}]", paths.join(" "))
}

/// In-memory sources keyed by their path relative to the import path (`loadDescriptors`).
struct MapResolver(HashMap<String, String>);

impl FileResolver for MapResolver {
    fn open_file(&self, name: &str) -> Result<File, protox::Error> {
        match self.0.get(name) {
            Some(src) => File::from_source(name, src),
            None => Err(protox::Error::file_not_found(name)),
        }
    }
}

/// Reads every `.proto` file under each import path and compiles them all into one pool.
pub fn load_descriptors(
    import_paths: &[PathBuf],
    confine: Option<&Path>,
) -> Result<DescriptorPool, String> {
    let mut files = HashMap::new();
    let mut total = 0u64;
    let confine = confine.map(|r| r.canonicalize().unwrap_or_else(|_| r.to_path_buf()));
    let mut walker = Walker {
        files: &mut files,
        total: &mut total,
        confine: confine.as_deref(),
    };
    for root in import_paths {
        walker.walk(root, root)?;
    }
    let mut names: Vec<String> = files.keys().cloned().collect();
    names.sort();
    let mut chain = ChainFileResolver::new();
    chain.add(MapResolver(files));
    chain.add(GoogleFileResolver::new());
    let mut compiler = protox::Compiler::with_file_resolver(chain);
    compiler.include_imports(true);
    compiler.open_files(&names).map_err(|e| e.to_string())?;
    Ok(compiler.descriptor_pool())
}

struct Walker<'a> {
    files: &'a mut HashMap<String, String>,
    total: &'a mut u64,
    confine: Option<&'a Path>,
}

impl Walker<'_> {
    fn walk(&mut self, root: &Path, dir: &Path) -> Result<(), String> {
        let meta = std::fs::metadata(dir).map_err(|e| format!("lstat {}: {e}", dir.display()))?;
        if meta.is_file() {
            return self.add_file(root, dir);
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("open {}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for p in entries {
            let ft = std::fs::symlink_metadata(&p)
                .map_err(|e| e.to_string())?
                .file_type();
            if ft.is_dir() {
                self.walk(root, &p)?;
            } else if p.extension().is_some_and(|e| e == "proto") {
                self.add_file(root, &p)?;
            }
        }
        Ok(())
    }

    fn add_file(&mut self, root: &Path, path: &Path) -> Result<(), String> {
        if let Some(confine) = self.confine {
            let real = path
                .canonicalize()
                .map_err(|e| format!("failed to read import {}: {e}", path.display()))?;
            if !real.starts_with(confine) {
                return Err(format!(
                    "import {} escapes the configured file root",
                    path.display()
                ));
            }
        }
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        *self.total = self.total.saturating_add(len);
        if *self.total > MAX_SCHEMA_BYTES {
            return Err(format!(
                "the .proto files under the import paths exceed {MAX_SCHEMA_BYTES} bytes"
            ));
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read import {}: {e}", path.display()))?;
        // A file named as the import path itself is relative to its own directory, as filepath.Rel gives ".".
        let rel = path
            .strip_prefix(root)
            .ok()
            .filter(|r| !r.as_os_str().is_empty());
        let name = match rel {
            Some(r) => r.to_string_lossy().replace('\\', "/"),
            None => ".".to_string(),
        };
        self.files.insert(name, content);
        Ok(())
    }
}

/// Fields of a message in declaration order (protojson `order.IndexNameFieldOrder`; extensions are unknown here).
fn declared_fields(md: &MessageDescriptor) -> Vec<FieldDescriptor> {
    md.descriptor_proto()
        .field
        .iter()
        .filter_map(|f| md.get_field(f.number() as u32))
        .collect()
}

/// Whether protojson writes the field without `emit_unpopulated` (`Message.Has`).
fn populated(m: &DynamicMessage, fd: &FieldDescriptor) -> bool {
    if fd.is_list() {
        return matches!(&*m.get_field(fd), PValue::List(l) if !l.is_empty());
    }
    if fd.is_map() {
        return matches!(&*m.get_field(fd), PValue::Map(map) if !map.is_empty());
    }
    if fd.supports_presence() {
        return m.has_field(fd);
    }
    *m.get_field(fd) != fd.default_value()
}

/// First required field left unset, by full name (`proto.CheckInitialized`).
fn check_required(m: &DynamicMessage) -> Result<(), String> {
    let md = m.descriptor();
    for fd in declared_fields(&md) {
        if fd.cardinality() == Cardinality::Required && !m.has_field(&fd) {
            return Err(fd.full_name().to_string());
        }
        if !m.has_field(&fd) {
            continue;
        }
        match &*m.get_field(&fd) {
            PValue::Message(sub) => check_required(sub)?,
            PValue::List(items) => {
                for v in items {
                    if let PValue::Message(sub) = v {
                        check_required(sub)?;
                    }
                }
            }
            PValue::Map(map) => {
                for v in map.values() {
                    if let PValue::Message(sub) = v {
                        check_required(sub)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn write_message(
    out: &mut Vec<u8>,
    m: &DynamicMessage,
    o: JsonOptions,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("exceeded maximum recursion depth".into());
    }
    let md = m.descriptor();
    if let Some(r) = write_well_known(out, m, &md, o, depth) {
        return r;
    }
    out.push(b'{');
    let mut first = true;
    for fd in declared_fields(&md) {
        let set = populated(m, &fd);
        if !set && !o.emit_unpopulated {
            continue;
        }
        // emit_unpopulated writes null for an unset singular message and for unset oneof members it skips.
        if !set && fd.containing_oneof().is_some() && !fd.is_list() && !fd.is_map() {
            continue;
        }
        if !first {
            out.push(b',');
        }
        first = false;
        let name = if o.use_proto_names {
            fd.name()
        } else {
            fd.json_name()
        };
        write_str(out, name)?;
        out.push(b':');
        if !set && !fd.is_list() && !fd.is_map() && matches!(fd.kind(), Kind::Message(_)) {
            out.extend_from_slice(b"null");
            continue;
        }
        let value = if set {
            m.get_field(&fd).into_owned()
        } else {
            fd.default_value()
        };
        write_field(out, &fd, &value, o, depth)?;
    }
    out.push(b'}');
    Ok(())
}

fn write_field(
    out: &mut Vec<u8>,
    fd: &FieldDescriptor,
    v: &PValue,
    o: JsonOptions,
    depth: usize,
) -> Result<(), String> {
    match v {
        PValue::List(items) => {
            out.push(b'[');
            let mut first = true;
            for item in items {
                if !first {
                    out.push(b',');
                }
                first = false;
                write_singular(out, &fd.kind(), item, o, depth)?;
            }
            out.push(b']');
        }
        PValue::Map(map) => {
            let value_kind = match fd.kind() {
                Kind::Message(entry) => entry.map_entry_value_field().kind(),
                k => k,
            };
            let mut entries: Vec<(&MapKey, &PValue)> = map.iter().collect();
            entries.sort_by(|a, b| map_key_order(a.0, b.0));
            out.push(b'{');
            for (i, (k, val)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_str(out, &map_key_text(k))?;
                out.push(b':');
                write_singular(out, &value_kind, val, o, depth)?;
            }
            out.push(b'}');
        }
        other => write_singular(out, &fd.kind(), other, o, depth)?,
    }
    Ok(())
}

/// `order.GenericKeyOrder`: false before true, numbers by value, strings bytewise.
fn map_key_order(a: &MapKey, b: &MapKey) -> std::cmp::Ordering {
    match (a, b) {
        (MapKey::Bool(x), MapKey::Bool(y)) => x.cmp(y),
        (MapKey::I32(x), MapKey::I32(y)) => x.cmp(y),
        (MapKey::I64(x), MapKey::I64(y)) => x.cmp(y),
        (MapKey::U32(x), MapKey::U32(y)) => x.cmp(y),
        (MapKey::U64(x), MapKey::U64(y)) => x.cmp(y),
        (MapKey::String(x), MapKey::String(y)) => x.as_bytes().cmp(y.as_bytes()),
        _ => std::cmp::Ordering::Equal,
    }
}

fn map_key_text(k: &MapKey) -> String {
    match k {
        MapKey::Bool(b) => b.to_string(),
        MapKey::I32(n) => n.to_string(),
        MapKey::I64(n) => n.to_string(),
        MapKey::U32(n) => n.to_string(),
        MapKey::U64(n) => n.to_string(),
        MapKey::String(s) => s.clone(),
    }
}

fn write_singular(
    out: &mut Vec<u8>,
    kind: &Kind,
    v: &PValue,
    o: JsonOptions,
    depth: usize,
) -> Result<(), String> {
    match v {
        PValue::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        PValue::I32(n) => out.extend_from_slice(n.to_string().as_bytes()),
        PValue::U32(n) => out.extend_from_slice(n.to_string().as_bytes()),
        PValue::I64(n) => write_str(out, &n.to_string())?,
        PValue::U64(n) => write_str(out, &n.to_string())?,
        PValue::F32(f) => write_float(out, f64::from(*f), true),
        PValue::F64(f) => write_float(out, *f, false),
        PValue::String(s) => write_str(out, s)?,
        PValue::Bytes(b) => write_str(out, &base64_std(b))?,
        PValue::EnumNumber(n) => match kind {
            Kind::Enum(e) if e.full_name() == "google.protobuf.NullValue" => {
                out.extend_from_slice(b"null")
            }
            Kind::Enum(e) => match e.get_value(*n) {
                Some(ev) => write_str(out, ev.name())?,
                None => out.extend_from_slice(n.to_string().as_bytes()),
            },
            _ => out.extend_from_slice(n.to_string().as_bytes()),
        },
        PValue::Message(m) => write_message(out, m, o, depth + 1)?,
        PValue::List(_) | PValue::Map(_) => return Err("nested list or map value".into()),
    }
    Ok(())
}

/// `internal/encoding/json.appendString`: escapes `"`, `\` and control characters only.
fn write_str(out: &mut Vec<u8>, s: &str) -> Result<(), String> {
    out.push(b'"');
    for c in s.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes())
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
    Ok(())
}

/// `internal/encoding/json.appendFloat`: ES6 number text, NaN and infinities as strings.
fn write_float(out: &mut Vec<u8>, n: f64, is32: bool) {
    out.extend_from_slice(go_float(n, is32).as_bytes());
}

/// Go's `strconv.AppendFloat(n, 'f' or 'e', -1, bits)` with encoding/json's exponent cut-offs.
pub fn go_float(n: f64, is32: bool) -> String {
    if n.is_nan() {
        return "\"NaN\"".into();
    }
    if n.is_infinite() {
        return if n > 0.0 {
            "\"Infinity\"".into()
        } else {
            "\"-Infinity\"".into()
        };
    }
    let abs = n.abs();
    let exp = abs != 0.0 && (abs < 1e-6 || abs >= 1e21);
    // Shortest digits that round-trip at the value's own width.
    let sci = if is32 {
        format!("{:e}", n as f32)
    } else {
        format!("{:e}", n)
    };
    let (mant, e) = sci.split_once('e').unwrap_or((&sci, "0"));
    let e: i32 = e.parse().unwrap_or(0);
    if exp {
        let sign = if e < 0 { '-' } else { '+' };
        let a = e.unsigned_abs();
        // Go writes at least two exponent digits, then encoding/json drops the leading zero of e-0X.
        return if e < 0 && a < 10 {
            format!("{mant}e{sign}{a}")
        } else {
            format!("{mant}e{sign}{a:02}")
        };
    }
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let point = e + 1; // digits before the decimal point
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    if point <= 0 {
        s.push_str("0.");
        s.extend(std::iter::repeat_n('0', point.unsigned_abs() as usize));
        s.push_str(&digits);
    } else if point as usize >= digits.len() {
        s.push_str(&digits);
        s.extend(std::iter::repeat_n('0', point as usize - digits.len()));
    } else {
        s.push_str(&digits[..point as usize]);
        s.push('.');
        s.push_str(&digits[point as usize..]);
    }
    s
}

fn base64_std(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        s.push(char::from(T[(n >> 18) as usize & 63]));
        s.push(char::from(T[(n >> 12) as usize & 63]));
        s.push(if chunk.len() > 1 {
            char::from(T[(n >> 6) as usize & 63])
        } else {
            '='
        });
        s.push(if chunk.len() > 2 {
            char::from(T[n as usize & 63])
        } else {
            '='
        });
    }
    s
}

fn field_by_name<'a>(m: &'a DynamicMessage, name: &str) -> Option<std::borrow::Cow<'a, PValue>> {
    m.get_field_by_name(name)
}

/// protojson's special forms of the well-known types (`well_known_types.go`); `None` for any other message.
fn write_well_known(
    out: &mut Vec<u8>,
    m: &DynamicMessage,
    md: &MessageDescriptor,
    o: JsonOptions,
    depth: usize,
) -> Option<Result<(), String>> {
    let name = md.full_name();
    let wrapper = matches!(
        name,
        "google.protobuf.BoolValue"
            | "google.protobuf.Int32Value"
            | "google.protobuf.Int64Value"
            | "google.protobuf.UInt32Value"
            | "google.protobuf.UInt64Value"
            | "google.protobuf.FloatValue"
            | "google.protobuf.DoubleValue"
            | "google.protobuf.StringValue"
            | "google.protobuf.BytesValue"
    );
    if wrapper {
        let fd = md.get_field(1)?;
        return Some(write_singular(out, &fd.kind(), &m.get_field(&fd), o, depth));
    }
    let r = match name {
        "google.protobuf.Empty" => {
            out.extend_from_slice(b"{}");
            Ok(())
        }
        "google.protobuf.Timestamp" => timestamp(m).map(|s| out.extend_from_slice(s.as_bytes())),
        "google.protobuf.Duration" => duration(m).map(|s| out.extend_from_slice(s.as_bytes())),
        "google.protobuf.FieldMask" => {
            let paths = match field_by_name(m, "paths").as_deref() {
                Some(PValue::List(l)) => l
                    .iter()
                    .filter_map(|v| v.as_str().map(lower_camel))
                    .collect::<Vec<_>>()
                    .join(","),
                _ => String::new(),
            };
            write_str(out, &paths)
        }
        "google.protobuf.Struct" | "google.protobuf.ListValue" => {
            let fd = md.get_field(1)?;
            write_field(out, &fd, &m.get_field(&fd), o, depth)
        }
        "google.protobuf.Value" => {
            let set = declared_fields(md).into_iter().find(|f| m.has_field(f));
            match set {
                None => Err(format!("{name}: none of the oneof fields is set")),
                Some(fd) => match &*m.get_field(&fd) {
                    PValue::F64(f) if !f.is_finite() => Err(format!("{name}: invalid {f} value")),
                    v => write_singular(out, &fd.kind(), v, o, depth),
                },
            }
        }
        "google.protobuf.Any" => write_any(out, m, md, o, depth),
        _ => return None,
    };
    Some(r)
}

/// `marshalAny`: `{"@type": url, <fields>}`, or `{"@type": url, "value": <json>}` for a well-known type.
fn write_any(
    out: &mut Vec<u8>,
    m: &DynamicMessage,
    md: &MessageDescriptor,
    o: JsonOptions,
    depth: usize,
) -> Result<(), String> {
    let name = md.full_name();
    let url = match field_by_name(m, "type_url").as_deref() {
        Some(PValue::String(u)) => u.clone(),
        _ => String::new(),
    };
    let value = match field_by_name(m, "value").as_deref() {
        Some(PValue::Bytes(b)) => b.clone(),
        _ => Default::default(),
    };
    if url.is_empty() {
        if value.is_empty() {
            out.extend_from_slice(b"{}");
            return Ok(());
        }
        return Err(format!("{name}: type_url is not set"));
    }
    let type_name = url.rsplit('/').next().unwrap_or(&url);
    // Bento's resolver holds top-level messages and their direct children only.
    let inner = md
        .parent_pool()
        .get_message_by_name(type_name)
        .filter(|t| {
            t.parent_message()
                .is_none_or(|p| p.parent_message().is_none())
        })
        .ok_or_else(|| format!("{name}: unable to resolve \"{url}\": \"not found\""))?;
    let inner = DynamicMessage::decode(inner, value).map_err(|_| {
        format!(
            "{name}: unable to unmarshal \"{url}\": proto: cannot parse invalid wire-format data"
        )
    })?;
    let mut body = Vec::new();
    write_message(&mut body, &inner, o, depth + 1)?;
    out.extend_from_slice(b"{\"@type\":");
    write_str(out, &url)?;
    let custom = body.first() != Some(&b'{') || is_well_known(inner.descriptor().full_name());
    if custom {
        out.extend_from_slice(b",\"value\":");
        out.extend_from_slice(&body);
        out.push(b'}');
    } else if body.len() > 2 {
        out.push(b',');
        out.extend_from_slice(&body[1..]);
    } else {
        out.push(b'}');
    }
    Ok(())
}

fn is_well_known(name: &str) -> bool {
    name.starts_with("google.protobuf.")
        && matches!(
            &name[16..],
            "Any"
                | "Timestamp"
                | "Duration"
                | "BoolValue"
                | "Int32Value"
                | "Int64Value"
                | "UInt32Value"
                | "UInt64Value"
                | "FloatValue"
                | "DoubleValue"
                | "StringValue"
                | "BytesValue"
                | "Struct"
                | "ListValue"
                | "Value"
                | "FieldMask"
                | "Empty"
        )
}

fn int_field(m: &DynamicMessage, name: &str) -> i64 {
    match field_by_name(m, name).as_deref() {
        Some(PValue::I64(n)) => *n,
        Some(PValue::I32(n)) => i64::from(*n),
        _ => 0,
    }
}

/// RFC 3339 in UTC with 0, 3, 6 or 9 fraction digits (`marshalTimestamp`).
fn timestamp(m: &DynamicMessage) -> Result<String, String> {
    let secs = int_field(m, "seconds");
    let nanos = int_field(m, "nanos");
    if !(-62_135_596_800..=253_402_300_799).contains(&secs) {
        return Err(format!(
            "google.protobuf.Timestamp: seconds out of range {secs}"
        ));
    }
    if !(0..1_000_000_000).contains(&nanos) {
        return Err(format!(
            "google.protobuf.Timestamp: nanos out of range {nanos}"
        ));
    }
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, mo, d) = civil_from_days(days);
    let mut s = format!(
        "\"{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}",
        tod / 3600,
        tod % 3600 / 60,
        tod % 60
    );
    s.push_str(&fraction(nanos));
    s.push_str("Z\"");
    Ok(s)
}

fn fraction(nanos: i64) -> String {
    if nanos == 0 {
        return String::new();
    }
    let f = format!("{nanos:09}");
    let f = f.trim_end_matches("000").trim_end_matches("000");
    format!(".{f}")
}

fn duration(m: &DynamicMessage) -> Result<String, String> {
    let secs = int_field(m, "seconds");
    let nanos = int_field(m, "nanos");
    if !(-315_576_000_000..=315_576_000_000).contains(&secs) {
        return Err(format!(
            "google.protobuf.Duration: seconds out of range {secs}"
        ));
    }
    if nanos <= -1_000_000_000
        || nanos >= 1_000_000_000
        || (secs > 0 && nanos < 0)
        || (secs < 0 && nanos > 0)
    {
        return Err(format!(
            "google.protobuf.Duration: nanos out of range {nanos}"
        ));
    }
    let neg = secs < 0 || nanos < 0;
    let mut s = String::from("\"");
    if neg {
        s.push('-');
    }
    s.push_str(&secs.unsigned_abs().to_string());
    s.push_str(&fraction(nanos.abs()));
    s.push_str("s\"");
    Ok(s)
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn lower_camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut up = false;
    for c in s.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_float_formats_like_encoding_json() {
        assert_eq!(go_float(5.0, false), "5");
        assert_eq!(go_float(0.1, true), "0.1");
        assert_eq!(go_float(-12.25, false), "-12.25");
        assert_eq!(go_float(1e21, false), "1e+21");
        assert_eq!(go_float(1e20, false), "100000000000000000000");
        assert_eq!(go_float(1e-7, false), "1e-7");
        assert_eq!(go_float(1.5e-10, false), "1.5e-10");
        assert_eq!(go_float(0.000001, false), "0.000001");
        assert_eq!(go_float(0.0, false), "0");
        assert_eq!(go_float(f64::NAN, false), "\"NaN\"");
        assert_eq!(go_float(f64::NEG_INFINITY, true), "\"-Infinity\"");
        assert_eq!(go_float(f64::from(60.123_f32), true), "60.123");
    }

    #[test]
    fn timestamps_and_durations_use_protojson_text() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(fraction(500_000_000), ".500");
        assert_eq!(fraction(1_000), ".000001");
        assert_eq!(fraction(1), ".000000001");
        assert_eq!(base64_std(b"ab"), "YWI=");
        assert_eq!(lower_camel("foo_bar_baz"), "fooBarBaz");
    }
}
