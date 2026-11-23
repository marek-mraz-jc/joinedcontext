#![forbid(unsafe_code)]
//! RDF representation, parsers, serializers and the bidirectional DCAT-AP mapping of
//! `dataroom` under the installed profiles of `crates/dataroom/profiles` (C7, C8, C17).

pub mod dcat;
pub mod graph;
pub mod jsonld;
pub mod profile;
pub mod rdfxml;
pub mod turtle;

pub use dcat::{
    accepted_profiles, catalogue_to_graph, dataset_to_graph, graph_to_datasets,
    negotiate_rdf_format, parse, parse_datasets_from_rdf, parse_rdf_graph, serialize_catalogue,
    serialize_dataset, HydraPageInfo, RdfFormat,
};
pub use graph::{ns, LiteralValue, RdfGraph, RdfLimits, SubjectNode, Term, Triple};
pub use profile::DcatProfile;
