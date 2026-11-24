//! Input: an uploaded SHACL shapes graph (Turtle), a 0 byte, then a data graph (Turtle).
//! Uploaded shapes compile or are refused under the upload limits; any data graph is validated
//! against them and against the built-in EU DCAT-AP shapes, never a panic, a stack overflow
//! or a hang, and the verdict agrees with its own counts.
#![no_main]

use dataroom_core::{CatalogueId, OrgId};
use dataroom_rdf::turtle::parse_turtle_bytes;
use dataroom_rdf::{RdfGraph, RdfLimits};
use dataroom_shacl::{
    parse_shapes_graph, validate_graph_against_shapes, ProfileRegistry, ShapesGraph,
};
use libfuzzer_sys::fuzz_target;
use std::sync::LazyLock;
use uuid::Uuid;

static EU: LazyLock<ShapesGraph> = LazyLock::new(|| {
    let scope = (
        OrgId::from_uuid(Uuid::from_u128(1)),
        CatalogueId::from_uuid(Uuid::from_u128(2)),
    );
    ProfileRegistry::new()
        .stack(scope, "dcat-ap-3.0.1", None)
        .expect("the EU profile is built in")
        .shapes
        .clone()
});

fn check(data: &RdfGraph, shapes: &ShapesGraph) {
    let report = validate_graph_against_shapes(
        data,
        shapes,
        OrgId::from_uuid(Uuid::from_u128(1)),
        CatalogueId::from_uuid(Uuid::from_u128(2)),
        "ds",
        "fuzz",
        "1",
    );
    assert_eq!(report.conforms, report.violation_count == 0);
    assert!(report.violation_count + report.warning_count <= report.findings.len());
}

fuzz_target!(|bytes: &[u8]| {
    let (shapes, data) = match bytes.iter().position(|b| *b == 0) {
        Some(at) => (&bytes[..at], &bytes[at + 1..]),
        None => (bytes, &[][..]),
    };
    let limits = RdfLimits {
        max_bytes: 64 * 1024,
        max_triples: 2_000,
        max_depth: 32,
        max_literal_length: 4_096,
        forbid_owl_imports: false,
    };
    let Ok(data) = parse_turtle_bytes(data, &limits) else {
        return;
    };
    check(&data, &EU);
    let Ok(shapes) = std::str::from_utf8(shapes) else {
        return;
    };
    if let Ok(uploaded) = parse_shapes_graph(shapes, &RdfLimits::uploaded_shapes()) {
        check(&data, &uploaded);
    }
});
