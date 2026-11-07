//! The bounds every input schema draws from (T-1659, AG-64, MF-40). The input schema is what an
//! MCP client reads in `tools/list`, what the assistant is prompted with, and what
//! [`call`](super::call) checks an input against before the operation's own parse: a property says what it is, a
//! string says how long it may be, and an object either names its properties or says how many
//! it may carry.
//!
//! Every operation file calls these leaves and `ops/mod.rs` calls every operation file, so
//! they call nothing back in `ops/mod.rs`: that would make each operation file a module cycle.

use serde_json::{json, Value};

use super::{OpError, OperationAnnotations};

/// A Kubernetes object name (DNS-1123 subdomain).
pub const NAME: u64 = 253;
/// A kind, a type, an attribute, a unit or a format name.
pub const TERM: u64 = 128;
/// An id: a URN, a change id, a key id.
pub const ID: u64 = 512;
/// A title or a short label a person reads.
pub const TITLE: u64 = 256;
/// A query expression.
pub const QUERY: u64 = 2048;
/// A URL.
pub const URL: u64 = 2048;
/// A message, a prompt, an answer: what a person or a model writes.
pub const TEXT: u64 = 20_000;
/// The top-level members of a manifest: apiVersion, kind, metadata, spec, status.
pub const MANIFEST_MEMBERS: u64 = 5;
/// The members of one free-form record: an entity, a sample row, a parameter map.
pub const RECORD_MEMBERS: u64 = 512;

/// The most one listing page of an operation carries, in bytes of its items' JSON (T-6291):
/// well under a connector's answer (`connectors::DEFAULT_LIMITS.result_bytes`) and a few
/// thousand tokens of a model's context, so a page reaches the client whole.
pub const PAGE_BYTES: usize = 32 * 1024;

/// How many of `items`, from the first, fit in `bytes` of their JSON: at least one when there
/// is one, so a single large item still pages on.
pub fn fitting<T: serde::Serialize>(items: &[T], bytes: usize) -> usize {
    let mut used = 0;
    items
        .iter()
        .take_while(|item| {
            used += serde_json::to_vec(item).map_or(bytes, |b| b.len());
            used <= bytes
        })
        .count()
        .max(1)
        .min(items.len())
}

/// A string property with its description and its length bound.
pub fn text(description: &str, max: u64) -> Value {
    json!({ "type": "string", "description": description, "maxLength": max })
}

/// A list of bounded strings.
pub fn texts(description: &str, max: u64) -> Value {
    json!({
        "type": "array",
        "description": description,
        "items": { "type": "string", "maxLength": max }
    })
}

/// A whole manifest as an object; the kind's own parse checks everything inside it (MF-37).
pub fn manifest(description: &str) -> Value {
    json!({
        "type": "object",
        "description": description,
        "maxProperties": MANIFEST_MEMBERS
    })
}

/// The saved draft to act on instead of an inline manifest.
pub fn draft_ref() -> Value {
    json!({
        "type": "object",
        "description": "A saved draft instead of a manifest",
        "properties": {
            "kind": text("Draft kind, e.g. Endpoint", TERM),
            "name": text("Draft metadata.name", NAME)
        },
        "required": ["kind", "name"],
        "additionalProperties": false
    })
}

/// The workspace a draft belongs to; absent is the project's own (CC-76, T-2267).
pub fn workspace() -> Value {
    text("The draft's workspace; absent for the project's own", NAME)
}

/// An operation's hints, in the order MCP names them.
pub(crate) fn annotations(
    read_only: bool,
    destructive: bool,
    idempotent: bool,
) -> OperationAnnotations {
    OperationAnnotations {
        read_only_hint: read_only,
        destructive_hint: destructive,
        idempotent_hint: idempotent,
    }
}

pub(crate) fn parse_input<T: serde::de::DeserializeOwned>(val: Value) -> Result<T, OpError> {
    serde_json::from_value(val).map_err(|e| {
        let (path, message) = serde_error_path_and_message(&e);
        OpError::InvalidInput { path, message }
    })
}

pub(crate) fn serde_error_path_and_message(err: &serde_json::Error) -> (String, String) {
    let msg = err.to_string();
    if let Some(rest) = msg.strip_prefix("unknown field `") {
        if let Some((field, _)) = rest.split_once('`') {
            return (format!("/{field}"), msg);
        }
    }
    if let Some(rest) = msg.strip_prefix("missing field `") {
        if let Some((field, _)) = rest.split_once('`') {
            return (format!("/{field}"), msg);
        }
    }
    ("/".to_string(), msg)
}

/// The verdict a check leaves on a draft, written out for the clients that read it (AG-62).
pub(crate) fn verdict_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "level": { "type": "string", "enum": ["error", "warning", "info"] },
                        "path": { "type": "string" },
                        "message": { "type": "string" }
                    }
                }
            },
            "trace": { "type": "object" },
            "checkedAt": { "type": "string", "format": "date-time" },
            "inputDigest": { "type": "string", "description": "`sha256:` and the digest of the manifest checked" }
        },
        "required": ["ok", "findings", "checkedAt", "inputDigest"]
    })
}

pub(crate) fn empty_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}
