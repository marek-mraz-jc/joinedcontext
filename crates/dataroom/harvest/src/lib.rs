#![forbid(unsafe_code)]
//! Catalogue harvesters (DCAT-AP, NKOD SPARQL, CKAN Action API, ArcGIS Hub) with SSRF guard,
//! content-hash idempotency, and withdrawal on complete read (C9).

pub mod guard;
pub mod runner;
pub mod sources;

pub use guard::{is_private_or_internal_ip, validate_outbound_url, MAX_HARVEST_BYTES};
pub use runner::{HarvestEngine, HttpFetcher, MapFetcher, OutboundFetcher};
