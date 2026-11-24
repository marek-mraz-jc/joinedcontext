//! Input: a Turtle document, then the same bytes as N-Triples. Each parse ends in a graph or a
//! named refusal under the limits, never a panic or a hang; a graph read goes back out through
//! the Turtle and N-Triples writers and is read again whole.
#![no_main]

use dataroom_rdf::turtle::{
    parse_ntriples_bytes, parse_turtle_bytes, serialize_ntriples, serialize_turtle,
};
use dataroom_rdf::RdfLimits;
use libfuzzer_sys::fuzz_target;

fn limits() -> RdfLimits {
    RdfLimits {
        max_bytes: 64 * 1024,
        max_triples: 2_000,
        max_depth: 32,
        max_literal_length: 4_096,
        forbid_owl_imports: true,
    }
}

fuzz_target!(|data: &[u8]| {
    let limits = limits();
    if let Ok(graph) = parse_turtle_bytes(data, &limits) {
        assert!(graph.len() <= limits.max_triples);
        let ttl = serialize_turtle(&graph);
        let back = parse_turtle_bytes(ttl.as_bytes(), &RdfLimits::default()).unwrap_or_else(|e| {
            panic!("the Turtle writer wrote what the reader refuses: {e}\n{ttl}")
        });
        assert_eq!(back.len(), graph.len(), "{ttl}");
    }
    if let Ok(graph) = parse_ntriples_bytes(data, &limits) {
        let nt = serialize_ntriples(&graph);
        let back = parse_ntriples_bytes(nt.as_bytes(), &RdfLimits::default()).unwrap_or_else(|e| {
            panic!("the N-Triples writer wrote what the reader refuses: {e}\n{nt}")
        });
        assert_eq!(back.len(), graph.len(), "{nt}");
    }
});
