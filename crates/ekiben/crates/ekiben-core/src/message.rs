use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use ekiben_bloblang::{DetailedErrorInfo, MessageBatch, PartMetaMut, Value};

use crate::error::{CoreError, DetailedError};
use crate::transaction::SyncResponseStore;

static NEXT_SORT_GROUP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortTag {
    pub group_id: u64,
    pub index: usize,
}

/// Associates message parts with their original indices in a batch (`internal/message/sort_group.go`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortGroup {
    pub id: u64,
    pub len: usize,
}

impl SortGroup {
    /// Creates a new `SortGroup` and returns `(group, tagged_batch)`.
    pub fn new(mut batch: Batch) -> (Self, Batch) {
        let id = NEXT_SORT_GROUP_ID.fetch_add(1, Ordering::Relaxed);
        let len = batch.len();
        for (i, part) in batch.iter_mut().enumerate() {
            part.sort_tags.push(SortTag {
                group_id: id,
                index: i,
            });
        }
        (Self { id, len }, batch)
    }

    /// Finds the original index of `part` in this `SortGroup`, or `None` if not affiliated.
    pub fn get_index(&self, part: &Part) -> Option<usize> {
        part.sort_tags
            .iter()
            .rev()
            .find(|t| t.group_id == self.id)
            .map(|t| t.index)
    }

    pub fn top_level_group(part: &Part) -> Option<SortGroup> {
        part.sort_tags.last().map(|t| SortGroup {
            id: t.group_id,
            len: 0,
        })
    }
}

/// A single message part (`internal/message/part.go` & `internal/message/data.go`).
///
/// Uses zero-copy `bytes::Bytes` for raw payloads, lazily parses structured JSON into `Option<Arc<Value>>`
/// on first structured read, and avoids re-serializing between consecutive structured processors.
#[derive(Clone, Debug)]
pub struct Part {
    raw_bytes: Option<Bytes>,
    structured: Option<Arc<Value>>,
    metadata: Arc<BTreeMap<String, Value>>,
    error: Option<DetailedError>,
    pub(crate) sort_tags: Vec<SortTag>,
    pub(crate) collapsed_count: usize,
    pub(crate) sync_response_store: Option<SyncResponseStore>,
}

pub type Batch = Vec<Part>;

impl Default for Part {
    fn default() -> Self {
        Self::new(Bytes::new())
    }
}

impl Part {
    pub fn new(data: impl Into<Bytes>) -> Self {
        Self {
            raw_bytes: Some(data.into()),
            structured: None,
            metadata: Arc::new(BTreeMap::new()),
            error: None,
            sort_tags: Vec::new(),
            collapsed_count: 1,
            sync_response_store: None,
        }
    }

    #[expect(
        clippy::should_implement_trait,
        reason = "infallible; FromStr would force a Result on every caller"
    )]
    pub fn from_str(s: &str) -> Self {
        Self::new(Bytes::copy_from_slice(s.as_bytes()))
    }

    pub fn from_structured(val: Value) -> Self {
        let mut p = Self::new(Bytes::new());
        p.set_structured(val);
        p
    }

    pub fn from_json(jv: serde_json::Value) -> Self {
        Self::from_structured(Value::from_json_value(jv))
    }

    /// Shallow copy: shares `Bytes`, `Arc<Value>`, and `Arc<BTreeMap>` CoW pointers (`Part.ShallowCopy`).
    pub fn shallow_copy(&self) -> Self {
        self.clone()
    }

    /// Deep copy: clones underlying buffers and maps (`Part.DeepCopy`).
    pub fn deep_copy(&self) -> Self {
        Self {
            raw_bytes: self
                .raw_bytes
                .as_ref()
                .map(|b| Bytes::copy_from_slice(b.as_ref())),
            structured: self.structured.as_ref().map(|v| Arc::new((**v).clone())),
            metadata: Arc::new((*self.metadata).clone()),
            error: self.error.clone(),
            sort_tags: self.sort_tags.clone(),
            collapsed_count: self.collapsed_count,
            sync_response_store: self.sync_response_store.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.raw_bytes.as_ref().is_none_or(|b| b.is_empty()) && self.structured.is_none()
    }

    /// Returns the raw byte representation, serializing structured data into JSON if needed (`Part.AsBytes`).
    pub fn as_bytes(&mut self) -> Bytes {
        if let Some(ref b) = self.raw_bytes {
            if !b.is_empty() || self.structured.is_none() {
                return b.clone();
            }
        }
        if let Some(ref s) = self.structured {
            let encoded = encode_structured_to_bytes(s);
            let b = Bytes::from(encoded);
            self.raw_bytes = Some(b.clone());
            return b;
        }
        Bytes::new()
    }

    /// Read-only byte access without mutating `&self` (computes JSON bytes on the fly if only structured is set).
    pub fn as_bytes_ref(&self) -> Bytes {
        if let Some(ref b) = self.raw_bytes {
            if !b.is_empty() || self.structured.is_none() {
                return b.clone();
            }
        }
        if let Some(ref s) = self.structured {
            return Bytes::from(encode_structured_to_bytes(s));
        }
        Bytes::new()
    }

    /// About how many bytes the part holds, without serializing a structured value: its raw
    /// length when it has one, otherwise a walk that counts each scalar's JSON text and the
    /// punctuation around it (escapes left out). For memory admission, where re-encoding every
    /// message before every processor cost a third of a heavy run (praha/green-management-4).
    pub fn size_estimate(&self) -> usize {
        if let Some(ref b) = self.raw_bytes {
            if !b.is_empty() || self.structured.is_none() {
                return b.len();
            }
        }
        self.structured.as_deref().map_or(0, json_len_estimate)
    }

    pub fn set_bytes(&mut self, data: impl Into<Bytes>) -> &mut Self {
        self.raw_bytes = Some(data.into());
        self.structured = None;
        self
    }

    pub fn set_structured(&mut self, val: Value) -> &mut Self {
        match val {
            Value::Null => {
                self.raw_bytes = Some(Bytes::from_static(b"null"));
                self.structured = Some(Arc::new(Value::Null));
            }
            Value::String(s) => {
                self.raw_bytes = None;
                self.structured = Some(Arc::new(Value::String(s)));
            }
            Value::Bytes(b) => {
                self.raw_bytes = Some(Bytes::from(b));
                self.structured = None;
            }
            other => {
                self.raw_bytes = None;
                self.structured = Some(Arc::new(other));
            }
        }
        self
    }

    /// Applies the result of a Bloblang mapping to `self` following `Executor.mapPart`:
    /// - `Value::String(s)` -> `SetBytes(s.as_bytes())` (raw unquoted string!)
    /// - `Value::Bytes(b)` -> `SetBytes(b)`
    /// - Any other `Value` -> `SetStructuredMut(val)`
    pub fn apply_mapped_value(&mut self, val: Value) {
        match val {
            Value::Nothing => {}
            Value::String(s) => {
                self.set_bytes(Bytes::from(s.into_bytes()));
            }
            Value::Bytes(b) => {
                self.set_bytes(Bytes::from(b));
            }
            other => {
                self.set_structured(other);
            }
        }
    }

    /// Lazily parses raw bytes as a single JSON document if not already structured (`Part.AsStructured`).
    /// Whether the part holds a structured value (a mapping's result) rather than only bytes a
    /// structured read would parse: Go's `%T` of a number tells the two apart (int64 or
    /// float64, json.Number).
    pub fn holds_structured(&self) -> bool {
        self.structured.is_some()
    }

    pub fn as_structured(&mut self) -> Result<Value, String> {
        if let Some(ref s) = self.structured {
            return Ok((**s).clone());
        }
        let val = self.as_structured_ref()?;
        self.structured = Some(Arc::new(val.clone()));
        Ok(val)
    }

    pub fn as_structured_ref(&self) -> Result<Value, String> {
        if let Some(ref s) = self.structured {
            return Ok((**s).clone());
        }
        let Some(ref raw) = self.raw_bytes else {
            return Err("target message part does not exist".to_string());
        };
        if raw.is_empty() {
            return Err("target message part does not exist".to_string());
        }
        decode_single_json_doc(raw)
    }

    pub fn as_structured_mut(&mut self) -> Result<&mut Value, String> {
        if self.structured.is_none() {
            let parsed = self.as_structured_ref()?;
            self.structured = Some(Arc::new(parsed));
        }
        self.raw_bytes = None;
        Ok(Arc::make_mut(self.structured.as_mut().unwrap()))
    }

    // Metadata operations (Copy-on-Write via `Arc::make_mut`)
    pub fn meta_get(&self, key: &str) -> Option<&Value> {
        self.metadata.get(key)
    }

    pub fn meta_get_str(&self, key: &str) -> String {
        self.metadata
            .get(key)
            .map(Value::to_string_coerced)
            .unwrap_or_default()
    }

    pub fn meta_set(&mut self, key: impl Into<String>, value: Value) {
        Arc::make_mut(&mut self.metadata).insert(key.into(), value);
    }

    pub fn meta_set_str(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.meta_set(key, Value::String(value.into()));
    }

    pub fn meta_delete(&mut self, key: &str) {
        Arc::make_mut(&mut self.metadata).remove(key);
    }

    pub fn meta_clear(&mut self) {
        Arc::make_mut(&mut self.metadata).clear();
    }

    pub fn meta_all(&self) -> &BTreeMap<String, Value> {
        &self.metadata
    }

    // Error flag operations
    pub fn error_get(&self) -> Option<&DetailedError> {
        self.error.as_ref()
    }

    pub fn error_set(&mut self, err: Option<DetailedError>) {
        self.error = err;
    }

    pub fn set_error_msg(&mut self, msg: impl Into<String>) {
        self.error = Some(DetailedError::plain(msg));
    }

    pub fn is_errored(&self) -> bool {
        self.error.is_some()
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    // Collapsed count tracking for `archive` (`internal/batch/count.go`)
    pub fn collapsed_count(&self) -> usize {
        self.collapsed_count
    }

    pub fn add_collapsed_count(&mut self, count: usize) {
        self.collapsed_count = self.collapsed_count.saturating_sub(1) + count;
    }

    pub fn with_sync_response_store(&mut self, store: SyncResponseStore) {
        self.sync_response_store = Some(store);
    }

    pub fn sync_response_store(&self) -> Option<&SyncResponseStore> {
        self.sync_response_store.as_ref()
    }
}

pub fn batch_collapsed_count(batch: &[Part]) -> usize {
    batch.iter().map(Part::collapsed_count).sum()
}

fn encode_structured_to_bytes(val: &Value) -> Vec<u8> {
    match val {
        Value::Bytes(b) => b.clone(),
        _ => val.to_go_json_unescaped().into_bytes(),
    }
}

fn decode_single_json_doc(raw: &[u8]) -> Result<Value, String> {
    if let Some(v) = ekiben_bloblang::go_json::decode_with_numbers(raw) {
        return Ok(v);
    }
    let mut de = serde_json::Deserializer::from_slice(raw).into_iter::<serde_json::Value>();
    let Some(first_res) = de.next() else {
        return Err("EOF".to_string());
    };
    let first = first_res.map_err(|e| e.to_string())?;
    if let Some(second_res) = de.next() {
        if second_res.is_ok() {
            return Err("message contains multiple valid documents".to_string());
        }
        return Err(second_res.unwrap_err().to_string());
    }
    Ok(Value::from_json_value(first))
}

/// Local wrapper type around `&'a [Part]` implementing `ekiben_bloblang::MessageBatch`.
///
/// Satisfies Section 4 Defect 1 (Rust orphan rule): never implement a foreign trait on `[Part]` or `Vec<Part>`.
pub struct PartsView<'a>(pub &'a [Part]);

impl<'a> MessageBatch for PartsView<'a> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn part_bytes(&self, index: usize) -> Cow<'_, [u8]> {
        let Some(part) = self.0.get(index) else {
            return Cow::Borrowed(&[]);
        };
        match (&part.raw_bytes, &part.structured) {
            (Some(b), s) if !b.is_empty() || s.is_none() => Cow::Borrowed(b.as_ref()),
            (_, Some(s)) => Cow::Owned(encode_structured_to_bytes(s)),
            _ => Cow::Borrowed(&[]),
        }
    }

    fn part_structured(&self, index: usize) -> Result<Value, String> {
        let Some(part) = self.0.get(index) else {
            return Err("message is empty".to_string());
        };
        part.as_structured_ref()
            .map_err(|e| format!("parse as json: {e}"))
    }

    fn part_meta_get(&self, index: usize, key: &str) -> Option<Value> {
        self.0.get(index).and_then(|p| p.meta_get(key).cloned())
    }

    fn part_meta_all(&self, index: usize) -> BTreeMap<String, Value> {
        self.0
            .get(index)
            .map(|p| p.meta_all().clone())
            .unwrap_or_default()
    }

    fn part_error(&self, index: usize) -> Option<DetailedErrorInfo> {
        self.0.get(index).and_then(|p| {
            p.error_get().map(|e| DetailedErrorInfo {
                message: e.message.clone(),
                label: e.label.clone(),
                component_type: e.component_type.clone(),
                path: e.path.clone(),
            })
        })
    }
}

/// Local wrapper around `&'a mut Part` implementing `ekiben_bloblang::PartMetaMut`.
pub struct PartMetaAdapter<'a>(pub &'a mut Part);

impl<'a> PartMetaMut for PartMetaAdapter<'a> {
    fn meta_get(&self, key: &str) -> Option<Value> {
        self.0.meta_get(key).cloned()
    }
    fn meta_set(&mut self, key: String, val: Value) {
        self.0.meta_set(key, val);
    }
    fn meta_delete(&mut self, key: &str) {
        self.0.meta_delete(key);
    }
    fn meta_clear(&mut self) {
        self.0.meta_clear();
    }
    fn meta_all(&self) -> BTreeMap<String, Value> {
        self.0.meta_all().clone()
    }
}

/// Bento binary batch format serializer (`internal/message/message.go:SerializeBytes`):
/// 4-byte BE part count, followed for each part by 4-byte BE part length + part bytes.
pub fn serialize_bytes(parts: &[&[u8]]) -> Vec<u8> {
    let total_len = 4 + parts.iter().map(|p| 4 + p.len()).sum::<usize>();
    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(&(parts.len() as u32).to_be_bytes());
    for p in parts {
        out.extend_from_slice(&(p.len() as u32).to_be_bytes());
        out.extend_from_slice(p);
    }
    out
}

/// Bento binary batch format deserializer (`internal/message/message.go:DeserializeBytes`).
pub fn deserialize_bytes(mut b: &[u8]) -> Result<Vec<Vec<u8>>, CoreError> {
    if b.len() < 4 {
        return Err(CoreError::other(
            "serialised message bytes were in unexpected format",
        ));
    }
    let num_parts = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
    if num_parts >= b.len() {
        return Err(CoreError::other(
            "serialised message bytes were in unexpected format",
        ));
    }
    b = &b[4..];
    let mut parts = Vec::with_capacity(num_parts);
    for _ in 0..num_parts {
        if b.len() < 4 {
            return Err(CoreError::other(
                "serialised message bytes were in unexpected format",
            ));
        }
        let part_size = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
        b = &b[4..];
        if b.len() < part_size {
            return Err(CoreError::other(
                "serialised message bytes were in unexpected format",
            ));
        }
        parts.push(b[..part_size].to_vec());
        b = &b[part_size..];
    }
    Ok(parts)
}

fn json_len_estimate(v: &Value) -> usize {
    match v {
        Value::Null | Value::Nothing | Value::Delete => 4,
        Value::Bool(_) => 5,
        Value::Int(i) => {
            i.unsigned_abs()
                .checked_ilog10()
                .map_or(1, |d| d as usize + 1)
                + 1
        }
        Value::Uint(u) => u.checked_ilog10().map_or(1, |d| d as usize + 1),
        Value::Float(_) => 24,
        Value::Number(t) => t.len(),
        Value::String(t) => t.len() + 2,
        Value::Bytes(b) => b.len() + 2,
        Value::Timestamp(_) => 32,
        Value::Array(items) => {
            items
                .iter()
                .map(|i| json_len_estimate(i) + 1)
                .sum::<usize>()
                + 2
        }
        Value::Object(map) => {
            map.iter()
                .map(|(k, i)| k.len() + 4 + json_len_estimate(i))
                .sum::<usize>()
                + 2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_structured_size_estimate_stays_near_its_json_length() {
        let v = Value::from_json_value(serde_json::json!({
            "id": "urn:ngsi-ld:Garden:x", "n": [1, -20, 3.5, true, null], "o": {"k": "v"}
        }));
        let p = Part::from_structured(v);
        let exact = p.as_bytes_ref().len();
        let estimate = p.size_estimate();
        assert!(
            estimate * 2 >= exact && estimate <= exact * 2,
            "{estimate} vs {exact}"
        );
        assert_eq!(Part::new(Bytes::from_static(b"abc")).size_estimate(), 3);
    }
}
