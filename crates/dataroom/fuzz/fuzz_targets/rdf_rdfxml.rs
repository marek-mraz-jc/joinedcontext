//! Input: an RDF/XML document. It ends in a graph or a named refusal (depth, attributes,
//! namespaces, entity declarations, bytes, triples), never a panic, a hang or an entity
//! expansion; a graph read goes back out through the RDF/XML writer and is read again.
#![no_main]

use dataroom_rdf::rdfxml::{parse_rdfxml_bytes, serialize_rdfxml};
use dataroom_rdf::RdfLimits;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = RdfLimits {
        max_bytes: 64 * 1024,
        max_triples: 2_000,
        max_depth: 32,
        max_literal_length: 4_096,
        forbid_owl_imports: true,
    };
    let Ok(graph) = parse_rdfxml_bytes(data, &limits) else {
        return;
    };
    assert!(graph.len() <= limits.max_triples);
    let xml = serialize_rdfxml(&graph);
    let back = parse_rdfxml_bytes(xml.as_bytes(), &RdfLimits::default())
        .unwrap_or_else(|e| panic!("the RDF/XML writer wrote what the reader refuses: {e}\n{xml}"));
    assert_eq!(back.len(), graph.len(), "{xml}");
});
