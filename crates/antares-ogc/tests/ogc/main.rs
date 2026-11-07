// SPDX-License-Identifier: EUPL-1.2
//! `antares-ogc` against a broker: one module per requirements class of OGC API - Features
//! Parts 1 and 5 (docs/analysis/ogc-features-mapping.md sections 3 and 6), sharing one fixture.

mod collections;
mod common;
mod core;
mod crs;
mod filter;
mod geojson;
mod history;
mod items;
mod matrix;
mod oas30;
mod outputs;
mod parquet;
mod scan;
mod schemas;
mod tabular;
mod tiles;
mod write;
