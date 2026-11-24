use std::collections::BTreeMap;
use std::fmt;

use jiff::{Timestamp, Zoned};
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
    Timestamp(Zoned),
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
            ) => self.as_f64() == other.as_f64(),
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

    pub fn to_timestamp_coerced(&self) -> Result<Zoned, String> {
        use crate::time_format::{fixed, utc};
        let unix = |secs: i64, nanos: i32| Timestamp::new(secs, nanos).ok().map(utc);
        match self {
            Value::Number(_) => self.sanitized().to_timestamp_coerced(),
            Value::Timestamp(ts) => Ok(ts.clone()),
            Value::Int(secs) => {
                unix(*secs, 0).ok_or_else(|| format!("invalid unix timestamp: {secs}"))
            }
            Value::Uint(secs) => i64::try_from(*secs)
                .ok()
                .and_then(|s| unix(s, 0))
                .ok_or_else(|| format!("invalid unix timestamp: {secs}")),
            Value::Float(secs) => {
                let whole = secs.trunc() as i64;
                let nanos = ((secs - (whole as f64)) * 1e9).round() as i64;
                let (whole, nanos) = if nanos < 0 {
                    (whole.saturating_sub(1), 1_000_000_000 + nanos)
                } else {
                    (whole, nanos)
                };
                // a fraction that rounds up to a whole second carries into it
                let (whole, nanos) = if nanos >= 1_000_000_000 {
                    (whole.saturating_add(1), nanos - 1_000_000_000)
                } else {
                    (whole, nanos)
                };
                i32::try_from(nanos)
                    .ok()
                    .and_then(|n| unix(whole, n))
                    .ok_or_else(|| format!("invalid unix float timestamp: {secs}"))
            }
            Value::String(s) => jc_time::parse_rfc3339_offset(s)
                .and_then(|(ts, off)| fixed(ts, off.seconds()).ok())
                .ok_or_else(|| format!("failed to parse RFC3339 timestamp '{s}': not RFC 3339")),
            Value::Bytes(b) => {
                let s = String::from_utf8_lossy(b);
                jc_time::parse_rfc3339_offset(&s)
                    .and_then(|(ts, off)| fixed(ts, off.seconds()).ok())
                    .ok_or_else(|| format!("failed to parse RFC3339 timestamp '{s}': not RFC 3339"))
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
                } else {
                    // without serde_json's arbitrary_precision every other number is an f64
                    n.as_f64().map_or(Value::Null, Value::Float)
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
pub fn format_rfc3339_nano(ts: &Zoned) -> String {
    crate::time_format::format_rfc3339_nano(ts)
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

/// Go's `%v` (`fmt.Sprint`) of a value as Bloblang holds it: `<nil>`, `[a b]`, `map[k:v]` with sorted keys.
pub fn format_go_v(v: &Value) -> String {
    match v {
        Value::Null | Value::Nothing | Value::Delete => "<nil>".to_string(),
        Value::Bytes(b) => format!(
            "[{}]",
            b.iter().map(u8::to_string).collect::<Vec<_>>().join(" ")
        ),
        // Go's Time.String: the fraction trimmed to its last non-zero digit.
        Value::Timestamp(ts) => {
            let frac = match ts.subsec_nanosecond() {
                0 => String::new(),
                n => format!(".{n:09}").trim_end_matches('0').to_string(),
            };
            let zone = match ts.offset().seconds() {
                0 => "+0000 UTC".to_string(),
                secs => {
                    let z = format!(
                        "{}{:02}{:02}",
                        if secs < 0 { '-' } else { '+' },
                        secs.abs() / 3600,
                        secs.abs() % 3600 / 60
                    );
                    format!("{z} {z}")
                }
            };
            format!("{}{frac} {zone}", ts.strftime("%Y-%m-%d %H:%M:%S"))
        }
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
        let a: Shared<Vec<Value>> = vec![Value::Int(1)].into();
        let mut b = a.clone();
        b.push(Value::Int(2));
        assert_eq!(Value::Array(a).to_go_json(), "[1]");
        assert_eq!(Value::Array(b).to_go_json(), "[1,2]");
    }

    fn path(p: &str) -> Vec<String> {
        p.split('.').map(str::to_string).collect()
    }

    fn epoch(offset_secs: i32) -> Zoned {
        let tz = jiff::tz::TimeZone::fixed(jiff::tz::Offset::from_seconds(offset_secs).unwrap());
        Timestamp::UNIX_EPOCH.to_zoned(tz)
    }

    #[test]
    fn a_shared_container_iterates_by_value_by_reference_and_mutably() {
        let mut a: Shared<Vec<Value>> = vec![Value::Int(1), Value::Int(2)].into();
        let keep = a.clone();
        for v in &mut a {
            *v = Value::Int(9);
        }
        assert_eq!((&keep).into_iter().count(), 2);
        assert_eq!(
            keep.into_iter().collect::<Vec<_>>(),
            [Value::Int(1), Value::Int(2)]
        );
        assert_eq!(
            a.into_iter().collect::<Vec<_>>(),
            [Value::Int(9), Value::Int(9)]
        );
    }

    #[test]
    fn type_names_and_equality_follow_bento_icompare() {
        assert_eq!(Value::Delete.type_name(), "delete");
        assert_eq!(Value::Nothing.type_name(), "nothing");
        assert_eq!(Value::Number("1".into()).type_name(), "number");
        assert!(Value::Delete.compare_eq(&Value::Delete));
        assert!(Value::Nothing.compare_eq(&Value::Nothing));
        assert!(!Value::Nothing.compare_eq(&Value::Null));
        assert!(Value::Timestamp(epoch(0)).compare_eq(&Value::Timestamp(epoch(0))));
        let later = Timestamp::new(1, 0)
            .unwrap()
            .to_zoned(jiff::tz::TimeZone::UTC);
        assert!(!Value::Timestamp(epoch(0)).compare_eq(&Value::Timestamp(later)));
        assert!(Value::Int(5).compare_eq(&Value::Uint(5)));
        assert!(!Value::Int(-1).compare_eq(&Value::Uint(u64::MAX)));
        assert!(Value::Uint(5).compare_eq(&Value::Int(5)));
        assert!(!Value::Uint(5).compare_eq(&Value::Int(-5)));
        assert!(Value::Float(2.0).compare_eq(&Value::Uint(2)));
        assert!(!Value::Float(2.5).compare_eq(&Value::Int(2)));
        assert!(Value::Number("7".into()).compare_eq(&Value::Int(7)));
        assert!(!Value::String("a".into()).compare_eq(&Value::Bytes(b"a".to_vec())));
    }

    #[test]
    fn sanitized_deep_reads_number_text_through_arrays_and_objects() {
        let mut m = BTreeMap::new();
        m.insert("n".to_string(), Value::Number("1.5".into()));
        m.insert("s".to_string(), Value::String("x".into()));
        let v = Value::Array(vec![Value::Number("3".into()), Value::Object(m.into())].into());
        assert!(v.holds_number_text());
        let clean = v.sanitized_deep();
        assert!(!clean.holds_number_text());
        assert_eq!(clean.to_go_json(), r#"[3,{"n":1.5,"s":"x"}]"#);
        assert!(!Value::Bool(true).holds_number_text());
        assert_eq!(Value::Bool(true).sanitized_deep(), Value::Bool(true));
        let mut only = BTreeMap::new();
        only.insert("n".to_string(), Value::Number("2".into()));
        assert!(Value::Object(only.into()).holds_number_text());
    }

    #[test]
    fn number_readers_convert_or_refuse_each_variant() {
        assert_eq!(Value::Number("2.5".into()).as_f64(), Some(2.5));
        assert_eq!(Value::Uint(3).as_f64(), Some(3.0));
        assert_eq!(Value::Number("42".into()).get_int(), Ok(42));
        assert_eq!(
            Value::Uint(u64::MAX).get_int().unwrap_err(),
            "uint64 value 18446744073709551615 exceeds int64 maximum"
        );
        assert_eq!(Value::Uint(7).get_int(), Ok(7));
        assert_eq!(
            Value::Float(1e19).get_int().unwrap_err(),
            "float value 10000000000000000000 exceeds int64 bounds"
        );
        assert_eq!(
            Value::Float(f64::NAN).get_int().unwrap_err(),
            "cannot convert non-finite float to int64"
        );
        assert_eq!(Value::Float(-3.0).get_int(), Ok(-3));
        assert_eq!(Value::Int(0).get_bool_go(), Ok(false));
        assert_eq!(Value::Float(0.5).get_bool_go(), Ok(true));
        assert_eq!(Value::Number("0".into()).get_bool_go(), Ok(false));
        assert_eq!(Value::Number("0.0".into()).get_bool_go(), Ok(true));
        assert!(Value::String("true".into()).get_bool_go().is_err());
    }

    #[test]
    fn coercions_to_text_and_numbers_cover_every_variant() {
        assert_eq!(Value::Float(0.1).to_string_coerced(), "0.1");
        assert_eq!(Value::Number("1e3".into()).to_string_coerced(), "1e3");
        assert_eq!(
            Value::Timestamp(epoch(0)).to_string_coerced(),
            "1970-01-01T00:00:00Z"
        );
        assert_eq!(Value::Number("2".into()).to_number_coerced(), Ok(2.0));
        assert_eq!(Value::Float(1.5).to_number_coerced(), Ok(1.5));
        assert_eq!(Value::Int(-2).to_number_coerced(), Ok(-2.0));
        assert_eq!(Value::Uint(2).to_number_coerced(), Ok(2.0));
        assert_eq!(
            Value::Uint(u64::MAX).to_int_coerced().unwrap_err(),
            "uint64 value 18446744073709551615 exceeds int64 maximum"
        );
        assert_eq!(Value::Bytes(b"0x10".to_vec()).to_int_coerced(), Ok(16));
        assert_eq!(
            Value::Null.to_int_coerced().unwrap_err(),
            "expected number or string value, got null"
        );
        assert_eq!(Value::Number("9".into()).to_uint_coerced(), Ok(9));
        assert!(Value::Number("1.5".into()).to_uint_coerced().is_err());
        assert_eq!(Value::Uint(4).to_uint_coerced(), Ok(4));
        assert_eq!(
            Value::Number("99999999999999999999".into())
                .to_uint_coerced()
                .unwrap_err(),
            r#"strconv.ParseInt: parsing "99999999999999999999": value out of range"#
        );
        assert_eq!(Value::Float(4.0).to_uint_coerced(), Ok(4));
        for bad in [f64::NAN, f64::INFINITY, -1.0, 1.5] {
            assert_eq!(
                Value::Float(bad).to_uint_coerced().unwrap_err(),
                format!("cannot convert float {bad} to uint64")
            );
        }
        assert_eq!(Value::Bytes(b"0b11".to_vec()).to_uint_coerced(), Ok(3));
        assert_eq!(
            Value::Bool(true).to_uint_coerced().unwrap_err(),
            "expected number or string value, got bool"
        );
    }

    #[test]
    fn bool_coercion_reads_numbers_text_and_go_spellings() {
        assert_eq!(Value::Number("0".into()).to_bool_coerced(), Ok(false));
        assert_eq!(Value::Bool(true).to_bool_coerced(), Ok(true));
        assert_eq!(Value::Int(2).to_bool_coerced(), Ok(true));
        assert_eq!(Value::Uint(0).to_bool_coerced(), Ok(false));
        assert_eq!(Value::Float(0.0).to_bool_coerced(), Ok(false));
        assert_eq!(Value::String("T".into()).to_bool_coerced(), Ok(true));
        assert_eq!(Value::Bytes(b"False".to_vec()).to_bool_coerced(), Ok(false));
        assert_eq!(
            Value::String("yes".into()).to_bool_coerced().unwrap_err(),
            "failed to parse 'yes' as bool"
        );
        assert_eq!(
            Value::Null.to_bool_coerced().unwrap_err(),
            "cannot convert null to bool"
        );
        for f in ["0", "f", "F", "false", "FALSE", "False"] {
            assert_eq!(parse_go_bool(f), Ok(false), "{f}");
        }
    }

    #[test]
    fn timestamp_coercion_reads_unix_numbers_and_rfc3339_text() {
        let fmt = |v: Value| v.to_timestamp_coerced().map(|t| format_rfc3339_nano(&t));
        assert_eq!(
            fmt(Value::Number("60".into())).as_deref(),
            Ok("1970-01-01T00:01:00Z")
        );
        assert_eq!(fmt(Value::Uint(1)).as_deref(), Ok("1970-01-01T00:00:01Z"));
        assert_eq!(
            fmt(Value::Int(i64::MAX)).unwrap_err(),
            "invalid unix timestamp: 9223372036854775807"
        );
        assert_eq!(
            fmt(Value::Uint(u64::MAX)).unwrap_err(),
            "invalid unix timestamp: 18446744073709551615"
        );
        assert_eq!(
            fmt(Value::Float(1.9999999999)).as_deref(),
            Ok("1970-01-01T00:00:02Z")
        );
        assert_eq!(
            fmt(Value::Float(-0.5)).as_deref(),
            Ok("1969-12-31T23:59:59.5Z")
        );
        assert_eq!(
            fmt(Value::Float(1e20)).unwrap_err(),
            "invalid unix float timestamp: 100000000000000000000"
        );
        assert_eq!(
            fmt(Value::Bytes(b"2024-01-02T03:04:05+01:00".to_vec())).as_deref(),
            Ok("2024-01-02T03:04:05+01:00")
        );
        assert_eq!(
            fmt(Value::Bytes(b"nope".to_vec())).unwrap_err(),
            "failed to parse RFC3339 timestamp 'nope': not RFC 3339"
        );
        assert_eq!(
            fmt(Value::Null).unwrap_err(),
            "expected timestamp, number, or string, got null"
        );
    }

    #[test]
    fn json_conversion_writes_whole_floats_as_go_and_drops_deleted_members() {
        assert_eq!(
            Value::Number("12".into()).to_json_value(),
            serde_json::json!(12)
        );
        assert_eq!(
            Value::Float(1.5e19).to_json_value(),
            serde_json::json!(15_000_000_000_000_000_000u64)
        );
        assert_eq!(
            Value::Float(f64::NAN).to_json_value(),
            serde_json::Value::Null
        );
        let mut m = BTreeMap::new();
        m.insert("gone".to_string(), Value::Nothing);
        m.insert("kept".to_string(), Value::Int(1));
        assert_eq!(
            Value::Object(m.into()).to_json_value(),
            serde_json::json!({"kept": 1})
        );
        let n: serde_json::Value = serde_json::from_str("18446744073709551616").unwrap();
        assert_eq!(
            Value::from_json_value(n),
            Value::Float(18446744073709551616.0)
        );
    }

    #[test]
    fn paths_read_arrays_by_index_and_refuse_what_is_not_there() {
        let v = Value::from_json_value(serde_json::json!({"a":[{"b":1},2],"s":"x"}));
        assert_eq!(v.get_path(&path("a.0.b")), Value::Int(1));
        assert_eq!(v.get_path(&path("a.5")), Value::Null);
        assert_eq!(v.get_path(&path("a.x")), Value::Null);
        assert_eq!(v.get_path(&path("s.t")), Value::Null);
        assert!(v.exists_path(&path("a.1")));
        assert!(!v.exists_path(&path("a.9")));
        assert!(!v.exists_path(&path("a.x")));
        assert!(!v.exists_path(&path("s.t")));
        assert!(!v.exists_path(&path("q")));
    }

    #[test]
    fn set_path_creates_objects_appends_and_names_the_failing_segment() {
        let mut v =
            Value::from_json_value(serde_json::json!({"a":{"n":null},"l":[{"x":1}],"s":"t"}));
        v.set_path(&path("a.n.m"), Value::Int(1)).unwrap();
        v.set_path(&path("l.-"), Value::Int(2)).unwrap();
        v.set_path(&path("l.-.k"), Value::Int(3)).unwrap();
        v.set_path(&path("l.0.y"), Value::Int(4)).unwrap();
        assert_eq!(
            v.to_go_json(),
            r#"{"a":{"n":{"m":1}},"l":[{"x":1,"y":4},2,{"k":3}],"s":"t"}"#
        );
        assert_eq!(
            v.set_path(&path("l.z"), Value::Null).unwrap_err(),
            "unable to set target path z: array index 'z' is not an integer"
        );
        assert_eq!(
            v.set_path(&path("l.7.q"), Value::Null).unwrap_err(),
            "unable to set target path 7.q: index 7 out of bounds"
        );
        assert_eq!(
            v.set_path(&path("s.t.u"), Value::Null).unwrap_err(),
            "unable to set target path s.t.u as the value of s was a non-object type (string)"
        );
        let mut root = Value::Int(1);
        assert_eq!(
            root.set_path(&path("a"), Value::Null).unwrap_err(),
            "unable to set target path a as the value of the root was a non-object type (number)"
        );
    }

    #[test]
    fn delete_path_removes_nested_keys_and_ignores_missing_ones() {
        let mut v = Value::from_json_value(serde_json::json!({"a":{"b":1,"c":2},"s":"t"}));
        v.delete_path(&path("a.b"));
        v.delete_path(&path("x.y"));
        v.delete_path(&path("s.t"));
        delete_path_recursive(&mut v, &[]);
        assert_eq!(v.to_go_json(), r#"{"a":{"c":2},"s":"t"}"#);
    }

    #[test]
    fn go_float_and_go_v_spell_specials_and_containers_as_go() {
        assert_eq!(format_go_float(f64::NAN), "NaN");
        assert_eq!(format_go_float(f64::INFINITY), "+Inf");
        assert_eq!(format_go_float(f64::NEG_INFINITY), "-Inf");
        assert_eq!(format_json_float(f64::INFINITY), "null");
        assert_eq!(format_go_v(&Value::Nothing), "<nil>");
        assert_eq!(format_go_v(&Value::Bytes(vec![1, 2])), "[1 2]");
        let v = Value::from_json_value(serde_json::json!({"a":[1,"x"],"b":null}));
        assert_eq!(format_go_v(&v), "map[a:[1 x] b:<nil>]");
        assert_eq!(
            type_error_from("x", &Value::Null, &["a", "b", "c"]),
            "expected a, b or c value, got null from x"
        );
    }

    #[test]
    fn go_integer_parsers_take_base_prefixes_and_refuse_overflow() {
        assert_eq!(
            parse_go_int("", 64).unwrap_err(),
            "cannot parse empty string as integer"
        );
        assert_eq!(parse_go_int("-0x10", 64), Ok(-16));
        assert_eq!(parse_go_int("+0b101", 64), Ok(5));
        assert_eq!(parse_go_int("0o17", 64), Ok(15));
        assert_eq!(parse_go_int("017", 64), Ok(15));
        assert_eq!(parse_go_int("-9223372036854775808", 64), Ok(i64::MIN));
        assert_eq!(
            parse_go_int("-9223372036854775809", 64).unwrap_err(),
            "integer '-9223372036854775809' overflows int64"
        );
        assert_eq!(parse_go_int("-1_000", 64), Ok(-1000));
        assert_eq!(
            parse_go_int("9223372036854775808", 64).unwrap_err(),
            "integer '9223372036854775808' overflows int64"
        );
        assert_eq!(
            check_int_bits(128, 8).unwrap_err(),
            "value 128 overflows int8"
        );
        assert_eq!(
            check_int_bits(-32769, 16).unwrap_err(),
            "value -32769 overflows int16"
        );
        assert_eq!(
            check_int_bits(1 << 31, 32).unwrap_err(),
            "value 2147483648 overflows int32"
        );
        assert_eq!(check_int_bits(-128, 8), Ok(-128));
        assert_eq!(
            parse_go_uint("-1", 64).unwrap_err(),
            "failed to parse '-1' as uint"
        );
        assert_eq!(
            parse_go_uint("", 64).unwrap_err(),
            "failed to parse '' as uint"
        );
        assert_eq!(parse_go_uint("+0B11", 64), Ok(3));
        assert_eq!(parse_go_uint("0O7", 64), Ok(7));
        assert_eq!(parse_go_uint("010", 64), Ok(8));
        assert_eq!(parse_go_uint("0xFF", 8), Ok(255));
        assert!(parse_go_uint("08", 64)
            .unwrap_err()
            .starts_with("failed to parse '08' as uint: "));
        assert_eq!(
            check_uint_bits(256, 8).unwrap_err(),
            "value 256 overflows uint8"
        );
        assert_eq!(
            check_uint_bits(65536, 16).unwrap_err(),
            "value 65536 overflows uint16"
        );
        assert_eq!(
            check_uint_bits(1 << 32, 32).unwrap_err(),
            "value 4294967296 overflows uint32"
        );
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
