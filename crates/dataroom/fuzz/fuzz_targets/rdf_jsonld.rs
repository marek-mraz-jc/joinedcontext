//! Input: a JSON-LD document. It ends in a graph or a named refusal (remote context, context
//! bomb, depth, bytes, triples, literal length) without any fetch, panic or hang; a graph read
//! goes back out through the JSON-LD writer and is read again.
#![no_main]

use dataroom_rdf::jsonld::{parse_jsonld_bytes, serialize_graph_jsonld};
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
    let Ok(graph) = parse_jsonld_bytes(data, &limits) else {
        return;
    };
    assert!(graph.len() <= limits.max_triples);
    let text = serialize_graph_jsonld(&graph);
    let back = parse_jsonld_bytes(text.as_bytes(), &RdfLimits::default()).unwrap_or_else(|e| {
        panic!("the JSON-LD writer wrote what the reader refuses: {e}\n{text}")
    });
    assert_eq!(back.len(), graph.len(), "{text}");
});
