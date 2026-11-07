#![forbid(unsafe_code)]
//! Catalogue harvesters (DCAT-AP, NKOD SPARQL, CKAN Action API, ArcGIS Hub) with SSRF guard,
//! content-hash idempotency, and withdrawal on complete read (C9).

pub mod guard;
pub mod runner;
pub mod sources;

pub use guard::{
    is_private_or_internal_ip, resolve_and_validate_host, validate_and_pin_url,
    validate_outbound_url, ValidatedTarget, MAX_HARVEST_BYTES, MAX_REDIRECT_HOPS,
};
pub use runner::{FetchResponse, HarvestEngine, HttpFetcher, MapFetcher, OutboundFetcher};
