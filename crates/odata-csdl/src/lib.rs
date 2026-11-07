// SPDX-License-Identifier: EUPL-1.2
//! OData 4.01 EDM model with CSDL XML and CSDL JSON writers (ADR-0024 decision 1, row
//! `odata-csdl`).
//!
//! A service builds one [`Model`] and writes it as `$metadata` in either representation; both
//! writers [`validate`](fn@validate) first, under the caller's [`Limits`], so an invalid or oversized model
//! is never half written. The subset is ADR-0024 decision 5: entity types (with one abstract
//! base), complex types, structural and navigation properties, terms, one entity container with
//! entity sets and navigation property bindings, and annotations whose values are constants,
//! paths, collections and records. Derived types beyond one abstract base, `Edm.Stream` and
//! CSDL JSON for a 4.0 model answer [`CsdlError::NotImplemented`]; actions, functions,
//! singletons, enum and type definitions have no field in the model at all.
//!
//! The crate knows nothing about its host (surfaces-threat-model 9.3, rule M1). It writes no
//! reader: no request path reads CSDL (Decided in T-3445, owner may override).
//!
//! `unsafe`: none here (`forbid`); the one dependency, `regex` (identifier classes of CSDL XML
//! 15.2, exactly `edm.xsd`'s `TSimpleIdentifier`), carries `unsafe` in its own source.
//!
//! ```
//! use odata_csdl::*;
//! let mut base = EntityType::new("Entity");
//! base.is_abstract = true;
//! base.key = vec!["id".into()];
//! base.properties.push(Property::new("id", TypeRef::single("Edm.String")).required());
//! let mut building = EntityType::new("Building");
//! building.base_type = Some("ns.Entity".into());
//! building.properties.push(Property::new("height", TypeRef::single("Edm.Double")));
//! let mut container = EntityContainer::new("Container");
//! container.entity_sets.push(EntitySet::new("Buildings", "ns.Building"));
//! let mut schema = Schema::new("ns");
//! schema.entity_types = vec![base, building];
//! schema.entity_container = Some(container);
//! let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![schema] };
//!
//! let (mut xml, mut json) = (Vec::new(), Vec::new());
//! write_xml(&model, &Limits::strict(), &mut xml)?;
//! write_json(&model, &Limits::strict(), &mut json)?;
//! assert!(String::from_utf8(xml)?.contains(r#"<EntitySet Name="Buildings" EntityType="ns.Building"/>"#));
//! assert!(String::from_utf8(json)?.contains(r#""$EntityContainer": "ns.Container""#));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]

mod error;
mod json;
mod model;
mod names;
mod validate;
mod xml;

#[cfg(test)]
mod tests;

pub use error::{CsdlError, Feature, LimitKind, Problem, ValidationError};
pub use json::write_json;
pub use model::{
    Annotation, ComplexType, CsdlVersion, EntityContainer, EntitySet, EntityType, Expr, Include,
    Model, NavigationProperty, NavigationPropertyBinding, Property, Reference,
    ReferentialConstraint, Scale, Schema, Srid, Term, TypeRef,
};
pub use names::{is_namespace, is_simple_identifier};
pub use validate::{validate, Limits};
pub use xml::write_xml;
