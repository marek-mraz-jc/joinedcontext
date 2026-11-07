#![forbid(unsafe_code)]
//! SHACL Core validator (`C8`) and versioned DCAT-AP profile registry (`C17`).

pub mod registry;
pub mod validator;

pub use registry::{built_in_profiles, derive_form_layout, ProfileRegistry};
pub use validator::{
    parse_shapes_graph, validate_graph_against_shapes, NodeShape, PathExpr, PropertyShape,
    ShapesGraph,
};
