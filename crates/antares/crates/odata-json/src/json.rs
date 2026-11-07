// SPDX-License-Identifier: EUPL-1.2
//! A request body to a `serde_json::Value` under [`BodyLimits`]: the bounds are counted while the
//! body is read, so an object with a million members stops at the first one over the bound, and
//! a member named twice is refused instead of the last one winning.

use std::cell::Cell;
use std::fmt;

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::Deserializer;
use serde_json::{Map, Value};

use crate::{BodyError, BodyLimit, BodyLimits};

/// Why the reader stopped, when it was not the JSON itself.
#[derive(Clone, Copy)]
enum Stop {
    Limit(BodyLimit, usize),
    Duplicate,
}

struct Budget<'l> {
    limits: &'l BodyLimits,
    members: Cell<usize>,
    stop: Cell<Option<Stop>>,
}

impl Budget<'_> {
    fn stop<E: serde::de::Error>(&self, stop: Stop) -> E {
        self.stop.set(Some(stop));
        E::custom("stopped")
    }

    fn limit<E: serde::de::Error>(&self, limit: BodyLimit, max: usize) -> E {
        self.stop(Stop::Limit(limit, max))
    }
}

/// One value at `depth` (the body itself is depth 0, its members 1).
#[derive(Clone, Copy)]
struct Node<'b, 'l> {
    budget: &'b Budget<'l>,
    depth: usize,
}

impl Node<'_, '_> {
    /// The node one container deeper, or the depth bound.
    fn child<E: serde::de::Error>(self) -> Result<Self, E> {
        let max = self.budget.limits.max_depth;
        match self.depth.checked_add(1) {
            Some(depth) if depth <= max => Ok(Self { depth, ..self }),
            _ => Err(self.budget.limit(BodyLimit::Depth, max)),
        }
    }
}

impl<'de> DeserializeSeed<'de> for Node<'_, '_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Node<'_, '_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let child = self.child()?;
        let max = self.budget.limits.max_collection_len;
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(child)? {
            if items.len() >= max {
                return Err(self.budget.limit(BodyLimit::CollectionLength, max));
            }
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let child = self.child()?;
        let max = self.budget.limits.max_properties;
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            match self.budget.members.get().checked_add(1) {
                Some(members) if members <= max => self.budget.members.set(members),
                _ => return Err(self.budget.limit(BodyLimit::Properties, max)),
            }
            if object.contains_key(&key) {
                return Err(self.budget.stop(Stop::Duplicate));
            }
            let value = map.next_value_seed(child)?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

/// `input` as one JSON value under `limits`. `serde_json` refuses nesting deeper than 128 on its
/// own (a syntax error here), whatever `max_depth` says.
pub(crate) fn parse(input: &[u8], limits: &BodyLimits) -> Result<Value, BodyError> {
    if input.len() > limits.max_bytes {
        return Err(BodyError::LimitExceeded {
            limit: BodyLimit::Bytes,
            max: limits.max_bytes,
        });
    }
    let budget = Budget {
        limits,
        members: Cell::new(0),
        stop: Cell::new(None),
    };
    let mut reader = serde_json::Deserializer::from_slice(input);
    let read = Node {
        budget: &budget,
        depth: 0,
    }
    .deserialize(&mut reader)
    .and_then(|value| reader.end().map(|()| value));
    read.map_err(|e| match budget.stop.get() {
        Some(Stop::Limit(limit, max)) => BodyError::LimitExceeded { limit, max },
        Some(Stop::Duplicate) => BodyError::DuplicateMember,
        None => BodyError::Syntax {
            line: e.line(),
            column: e.column(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::{BodyError, BodyLimit, BodyLimits};

    fn limits() -> BodyLimits {
        BodyLimits {
            max_bytes: 64,
            max_depth: 2,
            max_properties: 3,
            max_bind_links: 1,
            max_collection_len: 2,
            ..BodyLimits::strict()
        }
    }

    fn refused(input: &str) -> Option<BodyError> {
        parse(input.as_bytes(), &limits()).err()
    }

    #[test]
    fn every_json_value_reads() -> Result<(), BodyError> {
        let value = parse(br#"{"a":[true,-1],"b":{"c":"\u00e9"}}"#, &limits())?;
        assert_eq!(value["a"][1], -1);
        assert_eq!(value["b"]["c"], "é");
        let numbers = parse(b"[18446744073709551615, 1.5]", &limits())?;
        assert_eq!(numbers[0], u64::MAX);
        assert_eq!(numbers[1], 1.5);
        assert!(parse(b"null", &limits())?.is_null());
        Ok(())
    }

    #[test]
    fn bounds_and_duplicates_stop_the_read() {
        let limit = |limit, max| Some(BodyError::LimitExceeded { limit, max });
        assert_eq!(refused(&" ".repeat(65)), limit(BodyLimit::Bytes, 64));
        assert_eq!(refused(r#"{"a":{"b":[]}}"#), limit(BodyLimit::Depth, 2));
        assert_eq!(
            refused(r#"{"a":1,"b":2,"c":{"d":3}}"#),
            limit(BodyLimit::Properties, 3)
        );
        assert_eq!(refused("[1,2,3]"), limit(BodyLimit::CollectionLength, 2));
        assert_eq!(
            refused(r#"{"a":1,"a":1}"#),
            Some(BodyError::DuplicateMember)
        );
        assert_eq!(
            refused(r#"{"a":1} x"#),
            Some(BodyError::Syntax { line: 1, column: 9 })
        );
        assert_eq!(
            refused(r#"{"a" 1}"#),
            Some(BodyError::Syntax { line: 1, column: 6 })
        );
    }

    #[test]
    fn a_value_serde_json_never_reads_is_not_a_json_value() {
        use serde::de::value::{BytesDeserializer, Error};
        use serde::de::DeserializeSeed;
        let budget = super::Budget {
            limits: &limits(),
            members: std::cell::Cell::new(0),
            stop: std::cell::Cell::new(None),
        };
        let node = super::Node {
            budget: &budget,
            depth: 0,
        };
        let read = node.deserialize(BytesDeserializer::<Error>::new(b"x"));
        assert!(read.is_err_and(|e| e.to_string().contains("a JSON value")));
    }

    #[test]
    fn serde_json_s_own_depth_bound_is_a_syntax_error() {
        let deep = BodyLimits {
            max_depth: 1000,
            ..BodyLimits::strict()
        };
        let body = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert!(matches!(
            parse(body.as_bytes(), &deep),
            Err(BodyError::Syntax { .. })
        ));
    }
}
