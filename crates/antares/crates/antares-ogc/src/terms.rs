// SPDX-License-Identifier: EUPL-1.2
//! The names one collection's requests and answers use for its Attributes, shared by `items`
//! and the CQL2 `filter` so neither module reaches into the other for them.

use crate::model::Collection;
use serde_json::{Map, Value};
use std::collections::HashMap;

/// The terms one collection's requests use: core-context names as they are, every other
/// Attribute by its property name in the request's own `@context` (so an `ngsild.` option
/// names it as `properties` does), or `p{n}` when that name is no NGSI-LD term or the core
/// context already defines it; the answer's keys are then known.
pub(crate) struct Terms {
    pub(crate) context: Map<String, Value>,
    /// Each Attribute's term by IRI: looked up per property per Feature, so a map, not a
    /// scan of every term (T-6619 R3-018).
    pub(crate) by_iri: HashMap<String, String>,
}

impl Terms {
    pub(crate) fn of(c: &Collection) -> Self {
        let mut t = Self {
            context: Map::new(),
            by_iri: HashMap::with_capacity(c.properties.len().saturating_add(1)),
        };
        let span = c.properties.len().saturating_add(1);
        let named = c
            .geometry
            .iter()
            .map(|g| (g, None))
            .chain(c.properties.iter().map(|p| (&p.iri, Some(p.name.as_str()))));
        for (iri, name) in named {
            let core = antares_ops::core_name(iri);
            let term = if core == *iri {
                let free = |term: &str| {
                    is_term(term)
                        && !antares_ops::is_core_term(term)
                        && !t.context.contains_key(term)
                };
                let term = match name.filter(|n| free(n)) {
                    Some(name) => name.to_owned(),
                    // Each Attribute takes one name, so among as many `p{n}` past the
                    // terms so far as there are Attributes, one is free.
                    None => (t.context.len()..=t.context.len().saturating_add(span))
                        .map(|n| format!("p{n}"))
                        .find(|term| free(term))
                        .unwrap_or_default(),
                };
                t.context.insert(term.clone(), Value::from(iri.as_str()));
                term
            } else {
                core
            };
            // The first name an IRI takes stays, as a scan finds it first.
            t.by_iri.entry(iri.clone()).or_insert(term);
        }
        t
    }

    /// The term of an Attribute of the collection.
    pub(crate) fn term<'a>(&'a self, iri: &'a str) -> &'a str {
        self.by_iri.get(iri).map_or(iri, String::as_str)
    }

    /// The request's own `@context`, none when every term is the core context's.
    pub(crate) fn context(&self) -> Option<Value> {
        (!self.context.is_empty()).then(|| Value::Object(self.context.clone()))
    }
}

/// An Attribute name the NGSI-LD query grammar reads as one: a letter, then letters, digits
/// and `_` (TS 104 176 7.2.3.1 `attrName`, without the `%` escapes no slot name needs).
fn is_term(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && name.len() <= 256
}
