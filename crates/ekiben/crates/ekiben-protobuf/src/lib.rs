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
        // The entry's own type: a symlink is never followed into a directory.
        let mut entries: Vec<(PathBuf, std::fs::FileType)> = std::fs::read_dir(dir)
            .map_err(|e| format!("open {}: {e}", dir.display()))?
            .filter_map(|e| e.ok().and_then(|e| Some((e.path(), e.file_type().ok()?))))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (p, ft) in entries {
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
            let value_kind = fd
                .kind()
                .as_message()
                .map_or(fd.kind(), |entry| entry.map_entry_value_field().kind());
            let mut entries: Vec<(&MapKey, &PValue)> = map.iter().collect();
            entries.sort_by_key(|e| map_key_order(e.0));
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

/// `order.GenericKeyOrder` as a sort key (one map holds one key type): false before true,
/// numbers by value, strings bytewise.
fn map_key_order(k: &MapKey) -> (i128, &[u8]) {
    match k {
        MapKey::Bool(b) => (i128::from(*b), &[]),
        MapKey::I32(n) => (i128::from(*n), &[]),
        MapKey::I64(n) => (i128::from(*n), &[]),
        MapKey::U32(n) => (i128::from(*n), &[]),
        MapKey::U64(n) => (i128::from(*n), &[]),
        MapKey::String(s) => (0, s.as_bytes()),
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
        PValue::EnumNumber(n) => {
            let e = kind.as_enum();
            if e.is_some_and(|e| e.full_name() == "google.protobuf.NullValue") {
                out.extend_from_slice(b"null");
            } else if let Some(ev) = e.and_then(|e| e.get_value(*n)) {
                write_str(out, ev.name())?;
            } else {
                // A number the enum does not name.
                out.extend_from_slice(n.to_string().as_bytes());
            }
        }
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
        return if n.is_sign_positive() {
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
        let a = e.unsigned_abs();
        // Go writes at least two exponent digits, then encoding/json drops the leading zero of e-0X.
        return if e.is_negative() {
            format!("{mant}e-{a}")
        } else {
            format!("{mant}e+{a:02}")
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
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
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
        "google.protobuf.Timestamp" => timestamp(m).map(|s| out.extend_from_slice(s.as_bytes())),
        "google.protobuf.Duration" => duration(m).map(|s| out.extend_from_slice(s.as_bytes())),
        "google.protobuf.FieldMask" => {
            let field = field_by_name(m, "paths");
            let paths: Vec<String> = field
                .as_deref()
                .and_then(PValue::as_list)
                .unwrap_or_default()
                .iter()
                .filter_map(|v| v.as_str().map(lower_camel))
                .collect();
            write_str(out, &paths.join(","))
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
    let url = field_by_name(m, "type_url")
        .as_deref()
        .and_then(PValue::as_str)
        .unwrap_or_default()
        .to_string();
    let value = field_by_name(m, "value")
        .as_deref()
        .and_then(PValue::as_bytes)
        .cloned()
        .unwrap_or_default();
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

/// `seconds` (int64) or `nanos` (int32) of a Timestamp or Duration.
fn int_field(m: &DynamicMessage, name: &str) -> i64 {
    field_by_name(m, name)
        .as_deref()
        .and_then(|v| v.as_i64().or(v.as_i32().map(i64::from)))
        .unwrap_or(0)
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
    use serde_json::json;

    const PROTO: &str = r#"syntax = "proto2";
package t;
import "google/protobuf/any.proto";
import "google/protobuf/duration.proto";
import "google/protobuf/empty.proto";
import "google/protobuf/field_mask.proto";
import "google/protobuf/struct.proto";
import "google/protobuf/timestamp.proto";
import "google/protobuf/wrappers.proto";
message Inner { required string id = 1; }
message Bare {}
message Rec { optional Rec r = 1; }
message Outer { message Mid { message Deep {} } }
enum Color { RED = 0; GREEN = 1; }
message All {
  map<bool, int32> b = 1;
  map<int32, string> i32 = 2;
  map<int64, string> i64 = 3;
  map<uint32, string> u32 = 4;
  map<uint64, string> u64 = 5;
  map<string, Inner> s = 6;
  optional float f = 7;
  optional double d = 8;
  optional google.protobuf.Duration dur = 9;
  optional google.protobuf.Any any = 10;
  optional google.protobuf.Struct st = 11;
  optional google.protobuf.FieldMask fm = 12;
  optional google.protobuf.Int64Value w = 13;
  optional Color color = 14;
  repeated Inner list = 15;
  optional Inner one = 16;
  oneof pick { string a = 17; int32 n = 18; }
  optional google.protobuf.Value v = 19;
  optional google.protobuf.Empty e = 20;
  optional google.protobuf.ListValue lv = 21;
  optional google.protobuf.Timestamp ts = 22;
  optional string snake_name = 23;
  optional bytes raw = 24;
}
"#;

    fn schema() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("t.proto"), PROTO).expect("writes");
        dir
    }

    fn conf(config: JsonValue) -> ComponentConfig {
        ComponentConfig {
            label: String::new(),
            component_type: "protobuf".into(),
            config,
            processors: Vec::new(),
        }
    }

    fn processor(dir: &tempfile::TempDir, extra: JsonValue) -> ProtobufProcessor {
        let mut c = json!({"message": "t.All", "import_paths": [dir.path().to_str().unwrap()]});
        for (k, v) in extra.as_object().expect("object") {
            c[k] = v.clone();
        }
        ProtobufProcessor::from_config(&conf(c)).expect("builds")
    }

    /// `json` through from_json to wire format, then back through to_json with `opts`.
    fn round_trip(json: &str, opts: JsonValue) -> Result<String, String> {
        let dir = schema();
        let wire = processor(&dir, json!({"operator": "from_json"})).convert(json.as_bytes())?;
        let mut o = json!({"operator": "to_json"});
        for (k, v) in opts.as_object().expect("object") {
            o[k] = v.clone();
        }
        let out = processor(&dir, o).convert(&wire)?;
        Ok(String::from_utf8(out).expect("utf-8"))
    }

    fn pool() -> DescriptorPool {
        let dir = schema();
        load_descriptors(&[dir.path().to_path_buf()], None).expect("compiles")
    }

    fn msg(pool: &DescriptorPool, name: &str) -> DynamicMessage {
        DynamicMessage::new(pool.get_message_by_name(name).expect(name))
    }

    fn to_json(m: &DynamicMessage) -> Result<String, String> {
        let dir = schema();
        let p = processor(&dir, json!({"operator": "to_json"}));
        p.convert(&m.encode_to_vec())
            .map(|b| String::from_utf8(b).expect("utf-8"))
    }

    #[test]
    fn config_refusals_name_the_field() {
        let dir = schema();
        let d = dir.path().to_str().unwrap();
        let err = |c: JsonValue| {
            ProtobufProcessor::from_config(&conf(c))
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        assert!(err(json!([])).ends_with("protobuf config must be an object"));
        assert!(err(json!({"operator": "to_json", "message": "t.All", "import_paths": [d], "use_proto_names": 1}))
            .ends_with("protobuf field use_proto_names: expected bool value"));
        assert!(err(json!({"message": "t.All", "import_paths": [d]}))
            .ends_with("protobuf: field operator is required"));
        assert!(err(json!({"operator": "to_json", "import_paths": [d]}))
            .ends_with("protobuf: field message is required"));
        assert!(
            err(json!({"operator": "to_json", "message": "t.All", "import_paths": [1]}))
                .ends_with("protobuf field import_paths: expected strings")
        );
        assert!(
            err(json!({"operator": "to_json", "message": "t.All", "import_paths": d}))
                .ends_with("protobuf field import_paths: expected array")
        );
        // from_json finds top-level messages and their children, never a grandchild.
        assert!(err(
            json!({"operator": "from_json", "message": "t.Outer.Mid.Deep", "import_paths": [d]})
        )
        .ends_with(&format!(
            "unable to find message 't.Outer.Mid.Deep' definition within '[{d}]'"
        )));
        assert!(
            err(json!({"operator": "from_json", "message": "t.Nope", "import_paths": [d]}))
                .contains("unable to find message 't.Nope'")
        );
        let p = processor(
            &dir,
            json!({"operator": "from_json", "message": "t.Outer.Mid"}),
        );
        assert_eq!(p.processor_type(), "protobuf");
        assert_eq!(p.convert(b"{}"), Ok(Vec::new()));
        assert!(processor(
            &dir,
            json!({"operator": "to_json", "message": "t.Outer.Mid.Deep"})
        )
        .convert(b"")
        .is_ok());
    }

    #[test]
    fn maps_sort_their_keys_as_protojson_does() {
        let out = round_trip(
            r#"{"b":{"true":1,"false":2},"i32":{"2":"x","-1":"y"},"i64":{"10":"a","9":"b"},
               "u32":{"3":"c","1":"d"},"u64":{"5":"e","4":"f"},"s":{"b":{"id":"1"},"a":{"id":"2"}},
               "f":1.5,"d":0.25,"color":"GREEN","snakeName":"q\u0001\b\f\"\n\\\r\t","raw":"YWJj"}"#,
            json!({}),
        )
        .expect("converts");
        assert_eq!(
            out,
            r#"{"b":{"false":2,"true":1},"i32":{"-1":"y","2":"x"},"i64":{"9":"b","10":"a"},"u32":{"1":"d","3":"c"},"u64":{"4":"f","5":"e"},"s":{"a":{"id":"2"},"b":{"id":"1"}},"f":1.5,"d":0.25,"color":"GREEN","snakeName":"q\u0001\b\f\"\n\\\r\t","raw":"YWJj"}"#
        );
        let out =
            round_trip(r#"{"snakeName":"q"}"#, json!({"use_proto_names": true})).expect("converts");
        assert_eq!(out, r#"{"snake_name":"q"}"#);
    }

    #[test]
    fn well_known_types_take_their_protojson_forms() {
        let out = round_trip(
            r#"{"dur":"-1.5s","any":{"@type":"type.googleapis.com/t.Inner","id":"z"},
               "st":{"k":[1,"x",null,true]},"fm":"fooBar,baz","w":"7","v":2.5,"e":{},"lv":[1],
               "ts":"2024-01-02T03:04:05.123Z"}"#,
            json!({}),
        )
        .expect("converts");
        assert_eq!(
            out,
            r#"{"dur":"-1.500s","any":{"@type":"type.googleapis.com/t.Inner","id":"z"},"st":{"k":[1,"x",null,true]},"fm":"fooBar,baz","w":"7","v":2.5,"e":{},"lv":[1],"ts":"2024-01-02T03:04:05.123Z"}"#
        );
        let out = round_trip(
            r#"{"any":{"@type":"type.googleapis.com/google.protobuf.Duration","value":"2s"}}"#,
            json!({}),
        )
        .expect("converts");
        assert_eq!(
            out,
            r#"{"any":{"@type":"type.googleapis.com/google.protobuf.Duration","value":"2s"}}"#
        );
        let out = round_trip(
            r#"{"any":{"@type":"type.googleapis.com/google.protobuf.Struct","value":{"a":1}}}"#,
            json!({}),
        )
        .expect("converts");
        assert_eq!(
            out,
            r#"{"any":{"@type":"type.googleapis.com/google.protobuf.Struct","value":{"a":1}}}"#
        );
        let out = round_trip(
            r#"{"any":{"@type":"type.googleapis.com/t.Bare"}}"#,
            json!({}),
        )
        .expect("converts");
        assert_eq!(out, r#"{"any":{"@type":"type.googleapis.com/t.Bare"}}"#);
    }

    #[test]
    fn an_any_that_cannot_be_resolved_says_why() {
        let pool = pool();
        let any = |url: &str, value: &[u8]| {
            let mut a = msg(&pool, "google.protobuf.Any");
            a.set_field_by_name("type_url", PValue::String(url.into()));
            a.set_field_by_name("value", PValue::Bytes(value.to_vec().into()));
            let mut m = msg(&pool, "t.All");
            m.set_field_by_name("any", PValue::Message(a));
            to_json(&m)
        };
        assert_eq!(any("", b""), Ok(r#"{"any":{}}"#.into()));
        let e = "failed to unmarshal JSON protobuf message 't.All': google.protobuf.Any";
        assert_eq!(any("", b"\x08"), Err(format!("{e}: type_url is not set")));
        assert_eq!(
            any("x/t.Nope", b""),
            Err(format!(r#"{e}: unable to resolve "x/t.Nope": "not found""#))
        );
        assert_eq!(
            any("x/t.Outer.Mid.Deep", b""),
            Err(format!(
                r#"{e}: unable to resolve "x/t.Outer.Mid.Deep": "not found""#
            ))
        );
        assert_eq!(
            any("x/t.Inner", b"\xff"),
            Err(format!(
                r#"{e}: unable to unmarshal "x/t.Inner": proto: cannot parse invalid wire-format data"#
            ))
        );
    }

    #[test]
    fn timestamps_and_durations_out_of_range_are_refused() {
        let pool = pool();
        let wk = |name: &str, secs: i64, nanos: i32| {
            let mut m = msg(&pool, name);
            m.set_field_by_name("seconds", PValue::I64(secs));
            m.set_field_by_name("nanos", PValue::I32(nanos));
            m
        };
        let ts = "google.protobuf.Timestamp";
        assert_eq!(
            timestamp(&wk(ts, -62_135_596_800, 0)),
            Ok("\"0001-01-01T00:00:00Z\"".into())
        );
        assert_eq!(
            timestamp(&wk(ts, 253_402_300_799, 999_999_999)),
            Ok("\"9999-12-31T23:59:59.999999999Z\"".into())
        );
        assert_eq!(
            timestamp(&wk(ts, -62_135_596_801, 0)),
            Err(format!("{ts}: seconds out of range -62135596801"))
        );
        assert_eq!(
            timestamp(&wk(ts, 253_402_300_800, 0)),
            Err(format!("{ts}: seconds out of range 253402300800"))
        );
        assert_eq!(
            timestamp(&wk(ts, 0, -1)),
            Err(format!("{ts}: nanos out of range -1"))
        );
        assert_eq!(
            timestamp(&wk(ts, 0, 1_000_000_000)),
            Err(format!("{ts}: nanos out of range 1000000000"))
        );
        let du = "google.protobuf.Duration";
        assert_eq!(
            duration(&wk(du, 315_576_000_000, 0)),
            Ok("\"315576000000s\"".into())
        );
        assert_eq!(
            duration(&wk(du, -315_576_000_000, 0)),
            Ok("\"-315576000000s\"".into())
        );
        assert_eq!(duration(&wk(du, 0, -5_000)), Ok("\"-0.000005s\"".into()));
        assert_eq!(
            duration(&wk(du, 0, 999_999_999)),
            Ok("\"0.999999999s\"".into())
        );
        assert_eq!(
            duration(&wk(du, 315_576_000_001, 0)),
            Err(format!("{du}: seconds out of range 315576000001"))
        );
        assert_eq!(
            duration(&wk(du, -315_576_000_001, 0)),
            Err(format!("{du}: seconds out of range -315576000001"))
        );
        for (secs, nanos) in [(0, 1_000_000_000), (0, -1_000_000_000), (1, -1), (-1, 1)] {
            assert_eq!(
                duration(&wk(du, secs, nanos)),
                Err(format!("{du}: nanos out of range {nanos}")),
                "{secs} {nanos}"
            );
        }
    }

    #[test]
    fn a_value_needs_one_finite_member() {
        let pool = pool();
        let o = JsonOptions {
            use_proto_names: false,
            emit_unpopulated: false,
        };
        let mut out = Vec::new();
        let v = msg(&pool, "google.protobuf.Value");
        assert_eq!(
            write_message(&mut out, &v, o, 0),
            Err("google.protobuf.Value: none of the oneof fields is set".into())
        );
        let mut v = msg(&pool, "google.protobuf.Value");
        v.set_field_by_name("number_value", PValue::F64(f64::INFINITY));
        assert_eq!(
            write_message(&mut out, &v, o, 0),
            Err("google.protobuf.Value: invalid inf value".into())
        );
    }

    #[test]
    fn required_fields_are_checked_inside_lists_and_maps() {
        let pool = pool();
        let mut m = msg(&pool, "t.All");
        m.set_field_by_name(
            "list",
            PValue::List(vec![PValue::Message(msg(&pool, "t.Inner"))]),
        );
        let e = "failed to unmarshal protobuf message 't.All': proto: required field t.Inner.id not set";
        assert_eq!(to_json(&m), Err(e.into()));
        let mut m = msg(&pool, "t.All");
        m.set_field_by_name(
            "s",
            PValue::Map(
                [(
                    MapKey::String("k".into()),
                    PValue::Message(msg(&pool, "t.Inner")),
                )]
                .into(),
            ),
        );
        assert_eq!(to_json(&m), Err(e.into()));
        let mut m = msg(&pool, "t.All");
        m.set_field_by_name("one", PValue::Message(msg(&pool, "t.Inner")));
        assert_eq!(to_json(&m), Err(e.into()));
        let dir = schema();
        assert_eq!(
            processor(&dir, json!({"operator": "from_json"})).convert(br#"{"one":{}}"#),
            Err("failed to marshal protobuf message 't.All': proto: required field t.Inner.id not set".into())
        );
    }

    #[test]
    fn emit_unpopulated_writes_nulls_but_skips_unset_oneof_members() {
        let out = round_trip(r#"{"n":3}"#, json!({"emit_unpopulated": true})).expect("converts");
        assert!(out.contains(r#""one":null"#), "{out}");
        assert!(out.contains(r#""list":[]"#), "{out}");
        assert!(out.contains(r#""b":{}"#), "{out}");
        assert!(out.contains(r#""f":0"#), "{out}");
        assert!(out.contains(r#""n":3"#), "{out}");
        assert!(!out.contains(r#""a":"#), "{out}");
        let out = round_trip(r#"{"n":3}"#, json!({})).expect("converts");
        assert_eq!(out, r#"{"n":3}"#);
    }

    #[test]
    fn an_unknown_enum_number_is_written_as_a_number() {
        let pool = pool();
        let mut m = msg(&pool, "t.All");
        m.set_field_by_name("color", PValue::EnumNumber(7));
        assert_eq!(to_json(&m), Ok(r#"{"color":7}"#.into()));
    }

    #[test]
    fn nesting_past_the_depth_limit_is_refused() {
        // prost refuses more than 100 nested messages in one decode, but each Any's value is
        // decoded on its own: a chain of Anys reaches the writer's own limit.
        let pool = pool();
        let chain = |links: usize| {
            let mut m = msg(&pool, "t.All");
            for _ in 0..links {
                let mut a = msg(&pool, "google.protobuf.Any");
                a.set_field_by_name("type_url", PValue::String("x/t.All".into()));
                a.set_field_by_name("value", PValue::Bytes(m.encode_to_vec().into()));
                m = msg(&pool, "t.All");
                m.set_field_by_name("any", PValue::Message(a));
            }
            to_json(&m)
        };
        // Each link is two levels (the Any, then its message): 50 links stay at the limit of 100.
        assert!(chain(50).is_ok());
        assert_eq!(
            chain(51),
            Err("failed to unmarshal JSON protobuf message 't.All': exceeded maximum recursion depth".into())
        );
    }

    #[test]
    fn an_import_path_that_is_a_file_is_refused_and_subdirectories_are_read() {
        let dir = schema();
        let file = dir.path().join("t.proto");
        // Named "." as Bento's filepath.Rel names it, which the compiler then cannot open.
        let e = ProtobufProcessor::from_config(&conf(json!({
            "operator": "to_json", "message": "t.Inner", "import_paths": [file.to_str().unwrap()]
        })))
        .err()
        .map(|e| e.to_string());
        assert_eq!(
            e.as_deref(),
            Some("configuration error: file '.' is not in any include path")
        );
        let top = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(top.path().join("sub")).expect("mkdir");
        std::fs::write(top.path().join("sub/t.proto"), PROTO).expect("writes");
        std::fs::write(top.path().join("notes.txt"), "not a schema").expect("writes");
        let pool = load_descriptors(&[top.path().to_path_buf()], None).expect("compiles");
        assert!(pool.get_file_by_name("sub/t.proto").is_some());
        assert!(pool.get_message_by_name("t.All").is_some());
    }

    #[test]
    fn well_known_names_are_the_google_protobuf_types() {
        for t in [
            "Any",
            "Timestamp",
            "Duration",
            "BoolValue",
            "Int32Value",
            "Int64Value",
            "UInt32Value",
            "UInt64Value",
            "FloatValue",
            "DoubleValue",
            "StringValue",
            "BytesValue",
            "Struct",
            "ListValue",
            "Value",
            "FieldMask",
            "Empty",
        ] {
            assert!(is_well_known(&format!("google.protobuf.{t}")), "{t}");
            assert!(!is_well_known(&format!("t.{t}")), "{t}");
        }
        assert!(!is_well_known("google.protobuf.FileDescriptorProto"));
    }

    #[test]
    fn schema_files_that_cannot_be_read_are_refused() {
        // A file root that does not exist confines to itself: every real file escapes it.
        let dir = schema();
        let e = load_descriptors(
            &[dir.path().to_path_buf()],
            Some(Path::new("/nonexistent-ekiben-root")),
        )
        .err();
        assert_eq!(
            e,
            Some(format!(
                "import {} escapes the configured file root",
                dir.path().join("t.proto").display()
            ))
        );

        let missing = std::path::PathBuf::from("/nonexistent-ekiben-proto");
        assert!(load_descriptors(&[missing], None)
            .unwrap_err()
            .starts_with("lstat /nonexistent-ekiben-proto: "));

        let dir = tempfile::tempdir().expect("tempdir");
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).expect("mkdir");
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000))
            .expect("chmod");
        let r = load_descriptors(std::slice::from_ref(&locked), None);
        // root reads any directory: the refusal shows only where the directory is unreadable.
        let refused = std::fs::read_dir(&locked).is_err();
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("chmod back");
        assert_eq!(
            r.as_ref()
                .err()
                .map(|e| e.starts_with(&format!("open {}: ", locked.display()))),
            refused.then_some(true)
        );

        let dir = tempfile::tempdir().expect("tempdir");
        std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("a.proto"))
            .expect("symlink");
        assert!(
            load_descriptors(&[dir.path().to_path_buf()], Some(dir.path()))
                .unwrap_err()
                .starts_with("failed to read import ")
        );

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.proto"), b"\xff\xfe").expect("writes");
        assert!(load_descriptors(&[dir.path().to_path_buf()], None)
            .unwrap_err()
            .starts_with("failed to read import "));

        let dir = tempfile::tempdir().expect("tempdir");
        let big = std::fs::File::create(dir.path().join("a.proto")).expect("creates");
        // Exactly the cap is read (and fails as a schema, not on its size).
        big.set_len(MAX_SCHEMA_BYTES).expect("sparse file");
        let err = load_descriptors(&[dir.path().to_path_buf()], None).unwrap_err();
        assert!(!err.contains("exceed"), "{err}");
        big.set_len(MAX_SCHEMA_BYTES + 1).expect("sparse file");
        assert_eq!(
            load_descriptors(&[dir.path().to_path_buf()], None).err(),
            Some(format!(
                "the .proto files under the import paths exceed {MAX_SCHEMA_BYTES} bytes"
            ))
        );
    }

    /// Every day from 1600 to 2400 against a day-by-day proleptic Gregorian count.
    #[test]
    fn civil_from_days_walks_the_calendar() {
        let leap = |y: i64| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let (mut y, mut m, mut d) = (1600, 1, 1);
        let mut z = -135_140; // 1600-01-01
        while y < 2400 {
            assert_eq!(civil_from_days(z), (y, m, d), "day {z}");
            let last = match m {
                2 if leap(y) => 29,
                2 => 28,
                4 | 6 | 9 | 11 => 30,
                _ => 31,
            };
            d += 1;
            if d > last {
                (d, m) = (1, m + 1);
                if m > 12 {
                    (m, y) = (1, y + 1);
                }
            }
            z += 1;
        }
    }

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
        assert_eq!(base64_std(b"a"), "YQ==");
        assert_eq!(base64_std(b"abcd"), "YWJjZA==");
        assert_eq!(go_float(f64::INFINITY, false), "\"Infinity\"");
        assert_eq!(lower_camel("foo_bar_baz"), "fooBarBaz");
    }
}
