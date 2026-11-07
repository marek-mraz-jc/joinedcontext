#![forbid(unsafe_code)]
//! RDF representation, parsers, serializers and bidirectional DCAT-AP 3.0.1, DCAT-AP-CZ and
//! DCAT-AP-SK 2.0 mappings for `dataroom` (C7, C8, C17).

pub mod dcat;
pub mod graph;
pub mod jsonld;
pub mod rdfxml;
pub mod turtle;

pub use dcat::{
    catalogue_to_graph, dataset_to_graph, graph_to_datasets, negotiate_profile,
     negotiate_rdf_format, parse_datasets_from_rdf, parse_rdf_graph, serialize_catalogue,
    serialize_dataset, DcatProfileKind, HydraPageInfo, RdfFormat,
};
pub use graph::{LiteralValue, RdfGraph, RdfLimits, SubjectNode, Term, Triple, ns};
