// SPDX-License-Identifier: EUPL-1.2
//! OData 4.01 JSON format (ADR-0024 decision 1 and 5, row `odata-json`).
//!
//! - Writers for an entity, a collection, entity references, the service document and the
//!   error body, at `metadata=none|minimal|full`, in 4.0 or 4.01 spelling ([`Ctx`]).
//! - The body reader of the mapped writes: create, update and replace of an entity with
//!   single-valued binds, a property, a reference ([`read_entity_body`], [`read_property_body`],
//!   [`read_reference_body`]), bounded by the caller's [`BodyLimits`].
//! - `Prefer: odata.maxpagesize` ([`parse_prefer`]) and version negotiation
//!   ([`negotiate_version`]).
//!
//! Outside the subset: deep insert and deep update, collection-valued binds, unbinding, delta
//! payloads, `If-Match`, `$value` writes and collection appends answer
//! [`BodyError::NotImplemented`] or are refused by the surface before a body is read. The crate
//! knows nothing about Antares (`dev/check-module-rules.py`).
//!
//! ```
//! use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Property, Schema, TypeRef};
//! use odata_json::{negotiate_version, read_entity_body, write_entity, BodyLimits, Ctx, EntityValue, MetadataLevel, Resource, WriteKind};
//!
//! let mut city = Schema::new("City");
//! let mut building = EntityType::new("Building");
//! building.key.push("id".into());
//! building.properties.push(Property::new("id", TypeRef::single("Edm.String")).required());
//! city.entity_types.push(building);
//! let mut service = EntityContainer::new("Service");
//! service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
//! city.entity_container = Some(service);
//! let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] };
//!
//! let version = negotiate_version(None, Some("4.01")).map_err(|e| e.to_string())?;
//! let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::Minimal, version, metadata_etag: None };
//! let buildings = Resource::new(&model, "Buildings").ok_or("no set")?;
//! let created = read_entity_body(&ctx, &buildings, WriteKind::Create, br#"{"id":"b1"}"#, &BodyLimits::strict())
//!     .map_err(|e| e.to_string())?;
//! let mut body = Vec::new();
//! let entity = EntityValue { id: "Buildings('b1')".into(), properties: created.key };
//! write_entity(&ctx, &buildings, &entity, &mut body).map_err(|e| e.to_string())?;
//! assert_eq!(body, br#"{"@context":"https://example.org/odata/$metadata#Buildings/$entity","id":"b1"}"#);
//! # Ok::<(), String>(())
//! ```

#![forbid(unsafe_code)]

mod error;
mod format;
mod json;
mod model;
mod read;
mod write;

pub use error::{write_error, BodyError, BodyLimit, ErrorDetail, Feature, ODataError};
pub use format::{
    context_url, negotiate_version, parse_prefer, ContextTarget, Ctx, HeaderError, MetadataLevel,
    ODataVersion, Prefer,
};
pub use model::Resource;
pub use read::{
    read_entity_body, read_property_body, read_reference_body, Bind, BodyLimits, EntityPayload,
    Target, WriteKind,
};
pub use write::{
    write_collection, write_collection_end, write_collection_entity, write_collection_start,
    write_entity, write_entity_reference, write_entity_references, write_service_document,
    EntityValue, Page,
};
