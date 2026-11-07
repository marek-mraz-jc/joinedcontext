use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use smallvec::SmallVec;

/// An array or object body shared between clones and copied on the first write, as Go shares
/// the slices and maps a Bloblang value holds: `$features.filter(...)` per element or a `fold`
/// tally read field by field costs a reference count, not a deep copy of the document.
#[derive(Clone, Default, PartialEq)]
pub struct Shared<T>(std::sync::Arc<T>);

impl<T> std::ops::Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone> std::ops::DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        std::sync::Arc::make_mut(&mut self.0)
    }
}

impl<T: fmt::Debug> fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T> From<T> for Shared<T> {
    fn from(v: T) -> Self {
        Shared(std::sync::Arc::new(v))
    }
}

impl<T: Clone> Shared<T> {
    /// The body, without a copy when no other value shares it.
    pub fn into_inner(self) -> T {
        std::sync::Arc::unwrap_or_clone(self.0)
    }
}

impl<A, T: FromIterator<A>> FromIterator<A> for Shared<T> {
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        Shared::from(iter.into_iter().collect::<T>())
    }
}

impl<T: Clone + IntoIterator> IntoIterator for Shared<T> {
    type Item = T::Item;
    type IntoIter = T::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        self.into_inner().into_iter()
    }
}

impl<'a, T> IntoIterator for &'a Shared<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (&*self.0).into_iter()
    }
}

impl<'a, T: Clone> IntoIterator for &'a mut Shared<T>
where
    &'a mut T: IntoIterator,
{
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        std::sync::Arc::make_mut(&mut self.0).into_iter()
    }
}

/// A Bloblang runtime value, following Bento's `internal/value` semantics.
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    /// A JSON number read from a message as Go's `json.Number` (Bento decodes content with `UseNumber`),
    /// kept only where its text differs from what Go prints for the parsed value (`5.0`, `1E2`, an
    /// integer past int64): written back verbatim, computed with as [`Value::sanitized`] reads it.
    Number(String),
    String(String),
    Bytes(Vec<u8>),
    Timestamp(DateTime<FixedOffset>),
    Array(Shared<Vec<Value>>),
    Object(Shared<BTreeMap<String, Value>>),
    /// Returned by `deleted()`: deletes the target field, element, or message.
    Delete,
    /// Returned by `nothing()` or an unmatched `if`/`match`: skips the assignment.
    Nothing,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.compare_eq(other)
    }
}

impl Value {
    /// Returns the Bloblang type string as returned by `.type()`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) | Value::Uint(_) | Value::Float(_) | Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Bytes(_) => "bytes",
            Value::Timestamp(_) => "timestamp",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
            Value::Delete => "delete",
            Value::Nothing => "nothing",
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null | Value::Nothing)
    }

    /// Equality comparison following Bento's `value.ICompare`:
    /// Numbers are compared by numerical value across Int/Uint/Float.
    /// Other types must match both basic type and value (note: String and Bytes do not match each other in `ICompare`,
    /// though in arithmetic `==` Bento's `compareOp` coerces `Bytes` to `String` when LHS is `String`).
    pub fn compare_eq(&self, other: &Value) -> bool {
        if matches!(self, Value::Number(_)) || matches!(other, Value::Number(_)) {
            return self.sanitized().compare_eq(&other.sanitized());
        }
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Delete, Value::Delete) => true,
            (Value::Nothing, Value::Nothing) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            (Value::Timestamp(a), Value::Timestamp(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Uint(a), Value::Uint(b)) => a == b,
            (Value::Int(a), Value::Uint(b)) => *a >= 0 && (*a as u64) == *b,
            (Value::Uint(a), Value::Int(b)) => *b >= 0 && *a == (*b as u64),
            (
                Value::Float(_) | Value::Int(_) | Value::Uint(_),
                Value::Float(_) | Value::Int(_) | Value::Uint(_),
            ) => match (self.as_f64(), other.as_f64()) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            },
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.compare_eq(y))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(k, v)| b.get(k).is_some_and(|ov| v.compare_eq(ov)))
            }
            _ => false,
        }
    }

    /// Bento's `value.ISanitize` of a `json.Number`: an int64 when the text is one, else a float64.
    /// Any other value is returned as it is.
    pub fn sanitized(&self) -> Value {
        match self {
            Value::Number(t) => match t.parse::<i64>() {
                Ok(i) => Value::Int(i),
                Err(_) => t.parse::<f64>().map_or(Value::Null, Value::Float),
            },
            other => other.clone(),
        }
    }

    /// [`Value::sanitized`] without a copy of a value that is no `Number`.
    pub fn into_sanitized(self) -> Value {
        match self {
            Value::Number(_) => self.sanitized(),
            other => other,
        }
    }

    /// [`Value::sanitized`] through arrays and objects.
    pub fn sanitized_deep(self) -> Value {
        match self {
            Value::Number(_) => self.sanitized(),
            Value::Array(a) => Value::Array(a.into_iter().map(Value::sanitized_deep).collect()),
            Value::Object(m) => Value::Object(
                m.into_iter()
                    .map(|(k, v)| (k, v.sanitized_deep()))
                    .collect(),
            ),
            other => other,
        }
    }

    /// Whether a [`Value::Number`] sits anywhere in the value.
    pub fn holds_number_text(&self) -> bool {
        match self {
            Value::Number(_) => true,
            Value::Array(a) => a.iter().any(Value::holds_number_text),
            Value::Object(m) => m.values().any(Value::holds_number_text),
            _ => false,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(_) => self.sanitized().as_f64(),
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            Value::Uint(u) => Some(*u as f64),
            _ => None,
        }
    }

    pub fn get_number(&self) -> Result<f64, String> {
        self.as_f64().ok_or_else(|| {
            format!(
                "expected number value, got {} ({})",
                self.type_name(),
                self.to_display_short()
            )
        })
    }

    pub fn get_int(&self) -> Result<i64, String> {
        match self {
            Value::Number(_) => self.sanitized().get_int(),
            Value::Int(i) => Ok(*i),
            Value::Uint(u) => {
                i64::try_from(*u).map_err(|_| format!("uint64 value {u} exceeds int64 maximum"))
            }
            Value::Float(f) => {
                if f.is_nan() || f.is_infinite() {
                    return Err("cannot convert non-finite float to int64".to_string());
                }
                if f.fract() != 0.0 {
                    return Err(format!(
                        "failed to convert float value {f} containing decimals to integer"
                    ));
                }
                if *f < (i64::MIN as f64) || *f >= (i64::MAX as f64) {
                    return Err(format!("float value {f} exceeds int64 bounds"));
                }
                Ok(*f as i64)
            }
            _ => Err(format!("expected number value, got {}", self.type_name())),
        }
    }

    pub fn get_string(&self) -> Result<String, String> {
        match self {
            Value::String(s) => Ok(s.clone()),
            Value::Bytes(b) => Ok(String::from_utf8_lossy(b).into_owned()),
            _ => Err(format!("expected string value, got {}", self.type_name())),
        }
    }

    /// Bento's `value.IGetBool`, used by `&&` and `||`: a bool, or a number read as `!= 0`.
    pub fn get_bool_go(&self) -> Result<bool, String> {
        match self {
            Value::Bool(b) => Ok(*b),
            Value::Int(_) | Value::Uint(_) | Value::Float(_) => Ok(self.as_f64() != Some(0.0)),
            // IGetBool of a json.Number compares its text with "0".
            Value::Number(t) => Ok(t != "0"),
            other => Err(type_error_from("", other, &["bool"])),
        }
    }

    pub fn get_bool(&self) -> Result<bool, String> {
        match self {
            Value::Bool(b) => Ok(*b),
            _ => Err(format!("expected bool value, got {}", self.type_name())),
        }
    }

    /// Coerces any value to string (`value.IToString` in Bento).
    pub fn to_string_coerced(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            Value::Int(i) => i.to_string(),
            Value::Uint(u) => u.to_string(),
            Value::Float(f) => format_go_float(*f),
            Value::Number(t) => t.clone(),
            Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Value::Timestamp(ts) => format_rfc3339_nano(ts),
            Value::Null | Value::Nothing | Value::Delete => "null".to_string(),
            Value::Array(_) | Value::Object(_) => self.to_go_json(),
        }
    }

    /// Coerces any value to bytes (`value.IToBytes` in Bento).
    pub fn to_bytes_coerced(&self) -> Vec<u8> {
        match self {
            Value::Bytes(b) => b.clone(),
            Value::String(s) => s.as_bytes().to_vec(),
            _ => self.to_string_coerced().into_bytes(),
        }
    }

    /// Coerces a value to `f64` (`value.IToNumber` in Bento: `.number()`).
    pub fn to_number_coerced(&self) -> Result<f64, String> {
        match self {
            Value::Number(_) => self.sanitized().to_number_coerced(),
            Value::Float(f) => Ok(*f),
            Value::Int(i) => Ok(*i as f64),
            Value::Uint(u) => Ok(*u as f64),
            Value::String(s) => go_parse_float(s),
            Value::Bytes(b) => go_parse_float(&String::from_utf8_lossy(b)),
            _ => Err(format!(
                "expected number or string value, got {}",
                self.type_name()
            )),
        }
    }

    /// Coerces a value to `i64` (`value.IToInt` in Bento: `.int64()`).
    /// Uses Go's `strconv.ParseInt(s, 0, 64)` base-0 rules!
    /// Leading `0x`/`0X` = hex, `0b`/`0B` = binary, `0o`/`0O` or leading `0` = octal!
    /// Thus `"08"` and `"09"` fail as invalid octal, matching Bento's trap in PART B.
    pub fn to_int_coerced(&self) -> Result<i64, String> {
        match self {
            // IToInt of a json.Number is its Int64(), which refuses a fraction.
            Value::Number(t) => json_number_int64(t),
            Value::Int(i) => Ok(*i),
            Value::Uint(u) => {
                i64::try_from(*u).map_err(|_| format!("uint64 value {u} exceeds int64 maximum"))
            }
            Value::Float(_) => self.get_int(),
            Value::String(s) => parse_go_int(s, 64),
            Value::Bytes(b) => {
                let s = String::from_utf8_lossy(b);
                parse_go_int(&s, 64)
            }
            _ => Err(format!(
                "expected number or string value, got {}",
                self.type_name()
            )),
        }
    }

    pub fn to_uint_coerced(&self) -> Result<u64, String> {
        match self {
            Value::Number(t) => Value::Int(json_number_int64(t)?).to_uint_coerced(),
            Value::Uint(u) => Ok(*u),
            Value::Int(i) => {
                if *i < 0 {
                    Err(format!("cannot convert negative integer {i} to unsigned"))
                } else {
                    Ok(*i as u64)
                }
            }
            Value::Float(f) => {
                if f.is_nan() || f.is_infinite() || *f < 0.0 || f.fract() != 0.0 {
                    return Err(format!("cannot convert float {f} to uint64"));
                }
                Ok(*f as u64)
            }
            Value::String(s) => parse_go_uint(s, 64),
            Value::Bytes(b) => {
                let s = String::from_utf8_lossy(b);
                parse_go_uint(&s, 64)
            }
            _ => Err(format!(
                "expected number or string value, got {}",
                self.type_name()
            )),
        }
    }

    pub fn to_bool_coerced(&self) -> Result<bool, String> {
        match self {
            Value::Number(t) => Ok(t != "0"),
            Value::Bool(b) => Ok(*b),
            Value::Int(i) => Ok(*i != 0),
            Value::Uint(u) => Ok(*u != 0),
            Value::Float(f) => Ok(*f != 0.0),
            Value::String(s) => parse_go_bool(s),
            Value::Bytes(b) => parse_go_bool(&String::from_utf8_lossy(b)),
            _ => Err(format!("cannot convert {} to bool", self.type_name())),
        }
    }

    pub fn to_timestamp_coerced(&self) -> Result<DateTime<FixedOffset>, String> {
        match self {
            Value::Number(_) => self.sanitized().to_timestamp_coerced(),
            Value::Timestamp(ts) => Ok(*ts),
            Value::Int(secs) => Utc
                .timestamp_opt(*secs, 0)
                .single()
                .map(|dt| dt.fixed_offset())
                .ok_or_else(|| format!("invalid unix timestamp: {secs}")),
            Value::Uint(secs) => Utc
                .timestamp_opt(*secs as i64, 0)
                .single()
                .map(|dt| dt.fixed_offset())
                .ok_or_else(|| format!("invalid unix timestamp: {secs}")),
            Value::Float(secs) => {
                let whole = secs.trunc() as i64;
                let nanos = ((secs - (whole as f64)) * 1e9).round() as i64;
                let (whole, nanos) = if nanos < 0 {
                    (whole - 1, (1_000_000_000 + nanos) as u32)
                } else {
                    (whole, nanos as u32)
                };
                Utc.timestamp_opt(whole, nanos)
                    .single()
                    .map(|dt| dt.fixed_offset())
                    .ok_or_else(|| format!("invalid unix float timestamp: {secs}"))
            }
            Value::String(s) => DateTime::parse_from_rfc3339(s)
                .map_err(|e| format!("failed to parse RFC3339 timestamp '{s}': {e}")),
            Value::Bytes(b) => {
                let s = String::from_utf8_lossy(b);
                DateTime::parse_from_rfc3339(&s)
                    .map_err(|e| format!("failed to parse RFC3339 timestamp '{s}': {e}"))
            }
            _ => Err(format!(
                "expected timestamp, number, or string, got {}",
                self.type_name()
            )),
        }
    }

    pub fn to_display_short(&self) -> String {
        match self {
            Value::String(s) => format!("{s:?}"),
            _ => self.to_string_coerced(),
        }
    }

    pub fn from_json_value(v: serde_json::Value) -> Self {
        match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Int(i)
                } else if let Some(u) = n.as_u64() {
                    Value::Uint(u)
                } else if let Some(f) = n.as_f64() {
                    Value::Float(f)
                } else {
                    Value::Null
                }
            }
            serde_json::Value::String(s) => Value::String(s),
            serde_json::Value::Array(arr) => {
                Value::Array(arr.into_iter().map(Value::from_json_value).collect())
            }
            serde_json::Value::Object(map) => {
                let mut bmap = BTreeMap::new();
                for (k, val) in map {
                    bmap.insert(k, Value::from_json_value(val));
                }
                Value::Object(bmap.into())
            }
        }
    }

    pub fn to_json_value(&self) -> serde_json::Value {
        match self {
            Value::Null | Value::Delete | Value::Nothing => serde_json::Value::Null,
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Int(i) => serde_json::Value::Number((*i).into()),
            Value::Uint(u) => serde_json::Value::Number((*u).into()),
            Value::Number(_) => self.sanitized().to_json_value(),
            // Go's json.Marshal writes a whole float64 without a fraction (6, not 6.0); -0 stays.
            // ponytail: whole floats in [2^64, 1e21) still print as 1e20-style floats.
            Value::Float(f)
                if f.fract() == 0.0
                    && *f >= -9.223_372_036_854_776e18
                    && *f < 9.223_372_036_854_776e18
                    && !(*f == 0.0 && f.is_sign_negative()) =>
            {
                serde_json::Value::Number((*f as i64).into())
            }
            Value::Float(f) if f.fract() == 0.0 && *f >= 0.0 && *f < 1.844_674_407_370_955_2e19 => {
                serde_json::Value::Number((*f as u64).into())
            }
            Value::Float(f) => serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::String(s) => serde_json::Value::String(s.clone()),
            Value::Bytes(b) => {
                // In Go's encoding/json, []byte marshals as base64 when inside a JSON object/array!
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(b);
                serde_json::Value::String(encoded)
            }
            Value::Timestamp(ts) => serde_json::Value::String(format_rfc3339_nano(ts)),
            Value::Array(arr) => serde_json::Value::Array(
                arr.iter()
                    .filter(|v| !matches!(v, Value::Delete | Value::Nothing))
                    .map(Value::to_json_value)
                    .collect(),
            ),
            Value::Object(map) => {
                let mut jmap = serde_json::Map::with_capacity(map.len());
                for (k, v) in map {
                    if !matches!(v, Value::Delete | Value::Nothing) {
                        jmap.insert(k.clone(), v.to_json_value());
                    }
                }
                serde_json::Value::Object(jmap)
            }
        }
    }

    /// Traverses a path on `self` (like `gabs.Wrap(v).S(path...).Data()`).
    /// Returns `Value::Null` when a segment does not exist.
    pub fn get_path(&self, path: &[String]) -> Value {
        let mut curr = self;
        for seg in path {
            match curr {
                Value::Object(map) => match map.get(seg) {
                    Some(v) => curr = v,
                    None => return Value::Null,
                },
                Value::Array(arr) => {
                    if let Ok(idx) = seg.parse::<usize>() {
                        if let Some(v) = arr.get(idx) {
                            curr = v;
                        } else {
                            return Value::Null;
                        }
                    } else {
                        return Value::Null;
                    }
                }
                _ => return Value::Null,
            }
        }
        curr.clone()
    }

    pub fn exists_path(&self, path: &[String]) -> bool {
        let mut curr = self;
        for seg in path {
            match curr {
                Value::Object(map) => match map.get(seg) {
                    Some(v) => curr = v,
                    None => return false,
                },
                Value::Array(arr) => {
                    if let Ok(idx) = seg.parse::<usize>() {
                        if let Some(v) = arr.get(idx) {
                            curr = v;
                        } else {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }

    /// Sets a value at `path` inside `self`, creating intermediate objects as needed.
    /// Matches `JSONAssignment.Apply` and `gabs.Set`.
    pub fn set_path(&mut self, path: &[String], val: Value) -> Result<(), String> {
        if path.is_empty() {
            *self = val;
            return Ok(());
        }
        if matches!(self, Value::Null | Value::Nothing) {
            *self = Value::Object(BTreeMap::new().into());
        }
        set_path_recursive(self, path, &[], val)
    }

    /// Deletes the field at `path` inside `self`.
    pub fn delete_path(&mut self, path: &[String]) {
        if path.is_empty() {
            *self = Value::Delete;
            return;
        }
        delete_path_recursive(self, path);
    }
}

fn set_path_recursive(
    curr: &mut Value,
    remaining: &[String],
    visited: &[String],
    val: Value,
) -> Result<(), String> {
    let seg = &remaining[0];
    let tail = &remaining[1..];
    match curr {
        Value::Object(map) => {
            if tail.is_empty() {
                map.insert(seg.clone(), val);
                Ok(())
            } else {
                let mut next_visited = visited.to_vec();
                next_visited.push(seg.clone());
                let entry = map
                    .entry(seg.clone())
                    .or_insert_with(|| Value::Object(BTreeMap::new().into()));
                if matches!(entry, Value::Null | Value::Nothing) {
                    *entry = Value::Object(BTreeMap::new().into());
                }
                set_path_recursive(entry, tail, &next_visited, val)
            }
        }
        Value::Array(arr) => {
            let idx = if seg == "-" {
                arr.push(Value::Object(BTreeMap::new().into()));
                arr.len() - 1
            } else {
                seg.parse::<usize>().map_err(|_| {
                    format!(
                        "unable to set target path {}: array index '{}' is not an integer",
                        slice_to_dot_path(remaining),
                        seg
                    )
                })?
            };
            if idx >= arr.len() {
                return Err(format!(
                    "unable to set target path {}: index {} out of bounds",
                    slice_to_dot_path(remaining),
                    idx
                ));
            }
            if tail.is_empty() {
                arr[idx] = val;
                Ok(())
            } else {
                let mut next_visited = visited.to_vec();
                next_visited.push(seg.clone());
                set_path_recursive(&mut arr[idx], tail, &next_visited, val)
            }
        }
        other => {
            let full_path = {
                let mut p = visited.to_vec();
                p.extend_from_slice(remaining);
                slice_to_dot_path(&p)
            };
            if visited.is_empty() {
                Err(format!(
                    "unable to set target path {} as the value of the root was a non-object type ({})",
                    full_path,
                    other.type_name()
                ))
            } else {
                Err(format!(
                    "unable to set target path {} as the value of {} was a non-object type ({})",
                    full_path,
                    slice_to_dot_path(visited),
                    other.type_name()
                ))
            }
        }
    }
}

fn delete_path_recursive(curr: &mut Value, remaining: &[String]) {
    if remaining.is_empty() {
        return;
    }
    let seg = &remaining[0];
    let tail = &remaining[1..];
    if let Value::Object(map) = curr {
        if tail.is_empty() {
            map.remove(seg);
        } else if let Some(child) = map.get_mut(seg) {
            delete_path_recursive(child, tail);
        }
    }
}

/// Converts a dot path string to path segments, following `gabs.DotPathToSlice`:
/// splits on `.` and unescapes `~1` -> `.` and `~0` -> `~`.
pub fn dot_path_to_slice(path: &str) -> SmallVec<[String; 4]> {
    if path.is_empty() {
        return SmallVec::new();
    }
    path.split('.')
        .map(|part| part.replace("~1", ".").replace("~0", "~"))
        .collect()
}

/// gabs' `DotPathToSlice`, which Bento's `get`, `set`, `exists`, `with`, `without` and `explode`
/// take a path argument through: an empty path is the one key `""`, never the root.
pub fn gabs_path(path: &str) -> SmallVec<[String; 4]> {
    path.split('.')
        .map(|part| part.replace("~1", ".").replace("~0", "~"))
        .collect()
}

/// Escapes path segments for error messages / annotations (`query.SliceToDotPath`).
pub fn slice_to_dot_path(path: &[String]) -> String {
    path.iter()
        .map(|s| s.replace('~', "~0").replace('.', "~1"))
        .collect::<Vec<_>>()
        .join(".")
}

/// Escapes a quoted path segment so dots inside quotes are not split by `dot_path_to_slice`.
pub fn escape_quoted_path_segment(seg: &str) -> String {
    seg.replace('~', "~0").replace('.', "~1")
}

/// Formats a timestamp as Go's `time.RFC3339Nano` does: fraction digits without trailing zeros,
/// `Z` for UTC.
pub fn format_rfc3339_nano<Tz: chrono::TimeZone>(ts: &chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let full = ts.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    // `...:SS.nnnnnnnnn<offset>`: drop trailing zeros of the fraction, and the dot when none remain.
    match full.find('.') {
        Some(dot) => {
            let end = dot + 10;
            let frac = full[dot + 1..end].trim_end_matches('0');
            let sep = if frac.is_empty() { "" } else { "." };
            format!("{}{sep}{frac}{}", &full[..dot], &full[end..])
        }
        None => full,
    }
}

/// Formats a float64 the way Go's `strconv.FormatFloat(f, 'g', -1, 64)` does.
pub fn format_go_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f.is_sign_positive() {
            "+Inf".to_string()
        } else {
            "-Inf".to_string()
        };
    }
    let (_, exp) = shortest_digits(f);
    go_float(f, !(-4..6).contains(&exp))
}

/// Shortest round-trip decimal digits of `|f|` and the decimal exponent of the first digit. Ryu
/// breaks a tie between two shortest candidates to even as Go's strconv does (714.94000244140625
/// prints `714.9400024414062`); Rust's `{:e}` rounds it up.
fn shortest_digits(f: f64) -> (String, i32) {
    let mut buf = ryu::Buffer::new();
    let text = buf.format_finite(f.abs());
    let (mant, exp) = text.split_once('e').unwrap_or((text, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (int, frac) = mant.split_once('.').unwrap_or((mant, ""));
    let all = format!("{int}{frac}");
    let lead = all.bytes().take_while(|&b| b == b'0').count();
    let digits = all[lead..].trim_end_matches('0');
    if digits.is_empty() {
        return ("0".to_string(), 0);
    }
    // The first significant digit sits `int.len() - 1 - lead` places from the point.
    let first = int.len() as i32 - 1 - lead as i32 + exp;
    (digits.to_string(), first)
}

/// Go's `strconv.FormatFloat(f, 'e' or 'f', -1, 64)` for a finite `f`.
fn go_float(f: f64, exponent: bool) -> String {
    let (digits, exp) = shortest_digits(f);
    let mut out = String::new();
    if f.is_sign_negative() {
        out.push('-');
    }
    if f == 0.0 {
        out.push('0');
        return out;
    }
    if exponent {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push_str(&format!(
            "e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.unsigned_abs()
        ));
        return out;
    }
    let dp = exp + 1;
    if dp <= 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat(dp.unsigned_abs() as usize));
        out.push_str(&digits);
    } else if dp as usize >= digits.len() {
        out.push_str(&digits);
        out.push_str(&"0".repeat(dp as usize - digits.len()));
    } else {
        out.push_str(&digits[..dp as usize]);
        out.push('.');
        out.push_str(&digits[dp as usize..]);
    }
    out
}

/// Formats a float64 as Go's `encoding/json` does: `'f'` unless `|f| < 1e-6` or `|f| >= 1e21`, then `'e'`
/// with a one-digit negative exponent (`1e-7`). NaN and infinities, which Go refuses, become `null`.
pub fn format_json_float(f: f64) -> String {
    if !f.is_finite() {
        return "null".to_string();
    }
    let abs = f.abs();
    if (1e-6..1e21).contains(&abs) {
        // ryu prints the same shortest digits as Go; without an exponent its text is Go's 'f'
        // once a whole number's ".0" goes (a hot path: every float of every message).
        let mut buf = ryu::Buffer::new();
        let text = buf.format_finite(f);
        if !text.contains('e') {
            return text.strip_suffix(".0").unwrap_or(text).to_string();
        }
    }
    let mut s = go_float(f, abs != 0.0 && !(1e-6..1e21).contains(&abs));
    if let Some(pos) = s.find("e-0") {
        s.remove(pos + 2);
    }
    s
}

/// Bento's `value.NewTypeErrorFrom(from, v, expected...).Error()`:
/// `expected object or array value, got string from field `this` ("plain")`.
pub fn type_error_from(from: &str, v: &Value, expected: &[&str]) -> String {
    let mut out = String::from("expected ");
    for (i, e) in expected.iter().enumerate() {
        if i > 0 {
            out.push_str(if expected.len() > 2 && i < expected.len() - 1 {
                ", "
            } else {
                " or "
            });
        }
        out.push_str(e);
    }
    out.push_str(&format!(" value, got {}", v.type_name()));
    if !from.is_empty() {
        out.push_str(&format!(" from {from}"));
    }
    match v {
        Value::String(s) => out.push_str(&format!(" (\"{s}\")")),
        Value::Bool(_) | Value::Int(_) | Value::Uint(_) | Value::Float(_) => {
            out.push_str(&format!(" ({})", format_go_v(v)))
        }
        _ => {}
    }
    out
}

/// Go's `%s` of a value as Bloblang holds it, applied through arrays and maps: `%!s(<nil>)`, `%!s(bool=true)`;
/// numbers print as text (a JSON number reaches Bento as `json.Number`, a string type).
pub fn format_go_s(v: &Value) -> String {
    match v {
        Value::Null | Value::Nothing | Value::Delete => "%!s(<nil>)".to_string(),
        other => format_go_s_inner(other),
    }
}

/// Inside a slice or map Go prints a nil element as `<nil>` for every verb.
fn format_go_s_inner(v: &Value) -> String {
    match v {
        Value::Null | Value::Nothing | Value::Delete => "<nil>".to_string(),
        Value::Bool(b) => format!("%!s(bool={b})"),
        Value::Array(a) => format!(
            "[{}]",
            a.iter()
                .map(format_go_s_inner)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Value::Object(m) => format!(
            "map[{}]",
            m.iter()
                .map(|(k, v)| format!("{k}:{}", format_go_s_inner(v)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        other => other.to_string_coerced(),
    }
}

/// Go's `%v` (`fmt.Sprint`) of a value as Bloblang holds it: `<nil>`, `[a b]`, `map[k:v]` with sorted keys.
pub fn format_go_v(v: &Value) -> String {
    match v {
        Value::Null | Value::Nothing | Value::Delete => "<nil>".to_string(),
        Value::Bytes(b) => format!(
            "[{}]",
            b.iter().map(u8::to_string).collect::<Vec<_>>().join(" ")
        ),
        // ponytail: Go trims the fraction to its last non-zero digit; chrono prints 3, 6 or 9 digits.
        Value::Timestamp(ts) if ts.offset().local_minus_utc() == 0 => {
            ts.format("%Y-%m-%d %H:%M:%S%.f +0000 UTC").to_string()
        }
        Value::Timestamp(ts) => ts.format("%Y-%m-%d %H:%M:%S%.f %z %z").to_string(),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(format_go_v).collect::<Vec<_>>().join(" ")
        ),
        Value::Object(m) => format!(
            "map[{}]",
            m.iter()
                .map(|(k, v)| format!("{k}:{}", format_go_v(v)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        other => other.to_string_coerced(),
    }
}

/// Go's `strconv.ParseFloat(text, 64)` with its error text: no surrounding space, and a finite
/// spelling past f64 is out of range (Rust would read it as infinity). Hex floats are not read.
pub fn go_parse_float(text: &str) -> Result<f64, String> {
    let err = |why: &str| format!("strconv.ParseFloat: parsing {text:?}: {why}");
    let f = text.parse::<f64>().map_err(|_| err("invalid syntax"))?;
    let spelled_inf = text
        .trim_start_matches(['+', '-'])
        .to_ascii_lowercase()
        .starts_with("inf");
    if f.is_infinite() && !spelled_inf {
        return Err(err("value out of range"));
    }
    Ok(f)
}

/// Go's `json.Number.Int64()`: `strconv.ParseInt(text, 10, 64)` with its error text.
fn json_number_int64(text: &str) -> Result<i64, String> {
    text.parse::<i64>().map_err(|e| {
        let why = match e.kind() {
            std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
                "value out of range"
            }
            _ => "invalid syntax",
        };
        format!("strconv.ParseInt: parsing {text:?}: {why}")
    })
}

/// Parses an integer string using Go's `strconv.ParseInt(s, 0, bitSize)` rules.
pub fn parse_go_int(s: &str, bit_size: u32) -> Result<i64, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("cannot parse empty string as integer".to_string());
    }
    let (neg, rest) = if let Some(r) = trimmed.strip_prefix('-') {
        (true, r)
    } else if let Some(r) = trimmed.strip_prefix('+') {
        (false, r)
    } else {
        (false, trimmed)
    };
    let clean = rest.replace('_', "");
    let (radix, digits) = if let Some(h) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        (16, h)
    } else if let Some(b) = clean
        .strip_prefix("0b")
        .or_else(|| clean.strip_prefix("0B"))
    {
        (2, b)
    } else if let Some(o) = clean
        .strip_prefix("0o")
        .or_else(|| clean.strip_prefix("0O"))
    {
        (8, o)
    } else if clean.len() > 1 && clean.starts_with('0') {
        // Go base-0: leading '0' is octal! "08" and "09" fail here.
        (8, &clean[1..])
    } else {
        (10, clean.as_str())
    };

    let unsigned = u64::from_str_radix(digits, radix)
        .map_err(|e| format!("failed to parse '{s}' as integer: {e}"))?;

    let val: i64 = if neg {
        if unsigned == (1u64 << 63) {
            i64::MIN
        } else if unsigned > (i64::MAX as u64) {
            return Err(format!("integer '{s}' overflows int64"));
        } else {
            -(unsigned as i64)
        }
    } else {
        i64::try_from(unsigned).map_err(|_| format!("integer '{s}' overflows int64"))?
    };

    check_int_bits(val, bit_size)?;
    Ok(val)
}

pub fn check_int_bits(val: i64, bit_size: u32) -> Result<i64, String> {
    match bit_size {
        8 => i8::try_from(val)
            .map(i64::from)
            .map_err(|_| format!("value {val} overflows int8")),
        16 => i16::try_from(val)
            .map(i64::from)
            .map_err(|_| format!("value {val} overflows int16")),
        32 => i32::try_from(val)
            .map(i64::from)
            .map_err(|_| format!("value {val} overflows int32")),
        _ => Ok(val),
    }
}

pub fn parse_go_uint(s: &str, bit_size: u32) -> Result<u64, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return Err(format!("failed to parse '{s}' as uint"));
    }
    let rest = trimmed.strip_prefix('+').unwrap_or(trimmed);
    let clean = rest.replace('_', "");
    let (radix, digits) = if let Some(h) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        (16, h)
    } else if let Some(b) = clean
        .strip_prefix("0b")
        .or_else(|| clean.strip_prefix("0B"))
    {
        (2, b)
    } else if let Some(o) = clean
        .strip_prefix("0o")
        .or_else(|| clean.strip_prefix("0O"))
    {
        (8, o)
    } else if clean.len() > 1 && clean.starts_with('0') {
        (8, &clean[1..])
    } else {
        (10, clean.as_str())
    };
    let val = u64::from_str_radix(digits, radix)
        .map_err(|e| format!("failed to parse '{s}' as uint: {e}"))?;
    check_uint_bits(val, bit_size)
}

pub fn check_uint_bits(val: u64, bit_size: u32) -> Result<u64, String> {
    match bit_size {
        8 => u8::try_from(val)
            .map(u64::from)
            .map_err(|_| format!("value {val} overflows uint8")),
        16 => u16::try_from(val)
            .map(u64::from)
            .map_err(|_| format!("value {val} overflows uint16")),
        32 => u32::try_from(val)
            .map(u64::from)
            .map_err(|_| format!("value {val} overflows uint32")),
        _ => Ok(val),
    }
}

pub fn parse_go_bool(s: &str) -> Result<bool, String> {
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(format!("failed to parse '{s}' as bool")),
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_coerced())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_container_copies_on_write() {
        let a = Value::Array(vec![Value::Int(1)].into());
        let mut b = a.clone();
        if let Value::Array(items) = &mut b {
            items.push(Value::Int(2));
        }
        assert_eq!(a.to_go_json(), "[1]");
        assert_eq!(b.to_go_json(), "[1,2]");
    }

    #[test]
    fn json_float_fast_path_prints_what_go_float_prints() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        for i in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let f = (x >> 11) as f64 / (1u64 << 53) as f64 * 10f64.powi(i % 28 - 7);
            for f in [f, -f, f.round()] {
                let mut slow = go_float(f, f != 0.0 && !(1e-6..1e21).contains(&f.abs()));
                if let Some(pos) = slow.find("e-0") {
                    slow.remove(pos + 2);
                }
                assert_eq!(format_json_float(f), slow, "{f:e}");
            }
        }
    }

    #[test]
    fn go_parse_float_refuses_what_strconv_refuses() {
        assert_eq!(go_parse_float("1.5"), Ok(1.5));
        assert!(go_parse_float("-inf").unwrap().is_infinite());
        assert_eq!(
            go_parse_float(" 5").unwrap_err(),
            r#"strconv.ParseFloat: parsing " 5": invalid syntax"#
        );
        assert_eq!(
            go_parse_float("1e400").unwrap_err(),
            r#"strconv.ParseFloat: parsing "1e400": value out of range"#
        );
    }
}
