// SPDX-License-Identifier: EUPL-1.2
//! The LinkML subset the broker's surfaces use (odata-linkml-mapping.md
//! sections 8 and 9, ADR-0024): read from LinkML's resolved JSON
//! serialisation, never from YAML, with every metaslot outside the subset
//! refused (threat LK4); written as LinkML YAML, JSON Schema, a JSON-LD
//! `@context`, SHACL, OWL, RDFS, the RDF Data Cube and Markdown by
//! deterministic writers that quote and escape every name they print
//! (threat LK3).
//!
//! ```
//! use linkml_model::{Limits, Model};
//!
//! let json = br#"{
//!   "id": "https://example.org/building", "name": "building",
//!   "classes": {"Room": {"slots": ["temperature"]}},
//!   "slots": {"temperature": {"range": "float", "required": true}}
//! }"#;
//! let limits = Limits { max_bytes: 1 << 20, max_depth: 32, max_elements: 10_000 };
//! let model = Model::read(json, &limits).unwrap();
//! assert!(linkml_model::yaml(&model).contains("\"temperature\":"));
//! ```
#![forbid(unsafe_code)]

pub mod cache;
pub mod check;
mod error;
mod model;
mod schema;
mod write;

pub use error::Error;
pub use model::{
    edm_types, known_kind, Builtin, Limits, Model, Range, EDM_TYPE_TAG, FLAG_TAGS, KINDS, KIND_TAG,
    QB_COMPONENT_TAG, QB_DSD_TAG, SURFACE_LIMITS,
};
pub use schema::{
    normalize, Annotation, Class, Enum, PermissibleValue, Prefix, Schema, Slot, Unit,
    DEFAULT_RANGE, IMPORTS, NON_SEMANTIC,
};
pub use write::{context, json_schema, markdown, owl, qb, rdfs, shacl, yaml};
