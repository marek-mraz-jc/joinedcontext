//! Input: one page of a DCAT-AP feed in any RDF syntax; the first byte picks the Content-Type
//! the remote declared (or none). The page parses to records and a next link or an error, never
//! a panic or a hang.
#![no_main]

use dataroom_core::{CatalogueId, HarvestSource, OrgId};
use dataroom_harvest::sources::parse_dcat_page;
use libfuzzer_sys::fuzz_target;
use serde_json::json;
use uuid::Uuid;

const TYPES: [Option<&str>; 6] = [
    None,
    Some("text/turtle"),
    Some("application/rdf+xml"),
    Some("application/ld+json"),
    Some("application/n-triples"),
    Some("text/html"),
];

fuzz_target!(|data: &[u8]| {
    let Some((&pick, rest)) = data.split_first() else {
        return;
    };
    let Ok(body) = std::str::from_utf8(rest) else {
        return;
    };
    let source: HarvestSource = serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "hs-feed", "name": "feed",
        "kind": "dcat-ap", "url": "https://feeds.example.org/c.ttl", "publisherId": "city",
        "allowedHosts": ["feeds.example.org"], "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("source");
    let _ = parse_dcat_page(
        OrgId::from_uuid(Uuid::from_u128(1)),
        CatalogueId::from_uuid(Uuid::from_u128(10)),
        &source,
        body,
        TYPES[usize::from(pick) % TYPES.len()],
        "sk",
    );
});
