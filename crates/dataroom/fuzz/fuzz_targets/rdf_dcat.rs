//! Input: one byte that picks the format (Turtle, JSON-LD, RDF/XML, N-Triples), then a
//! harvested DCAT feed. Reading it ends in datasets or a refusal, never a panic; every dataset
//! read goes back out as Turtle under EU DCAT-AP and that Turtle parses.
#![no_main]

use dataroom_core::{Catalogue, CatalogueId, OrgId};
use dataroom_rdf::{parse_datasets_from_rdf, serialize_dataset, DcatProfile, RdfFormat, RdfLimits};
use libfuzzer_sys::fuzz_target;
use std::sync::LazyLock;
use uuid::Uuid;

static CATALOGUE: LazyLock<Catalogue> = LazyLock::new(|| {
    serde_json::from_value(serde_json::json!({
        "orgId": Uuid::from_u128(1), "id": Uuid::from_u128(2), "name": "city",
        "host": "data.city.example", "title": {"en": "City"}, "description": {"en": "Desc"},
        "defaultLanguage": "en", "languages": ["en"], "mainProfile": "dcat-ap-3.0.1",
        "activeProfiles": [], "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("catalogue fixture")
});

fuzz_target!(|data: &[u8]| {
    let Some((pick, rest)) = data.split_first() else {
        return;
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let format = match pick % 4 {
        0 => RdfFormat::Turtle,
        1 => RdfFormat::JsonLd,
        2 => RdfFormat::RdfXml,
        _ => RdfFormat::NTriples,
    };
    let limits = RdfLimits {
        max_bytes: 64 * 1024,
        max_triples: 2_000,
        max_depth: 32,
        max_literal_length: 4_096,
        forbid_owl_imports: true,
    };
    let org = OrgId::from_uuid(Uuid::from_u128(1));
    let cat = CatalogueId::from_uuid(Uuid::from_u128(2));
    let Ok(datasets) = parse_datasets_from_rdf(text, format, org, cat, "city", "en", &limits)
    else {
        return;
    };
    for dataset in &datasets {
        let ttl = serialize_dataset(
            dataset,
            &CATALOGUE,
            None,
            RdfFormat::Turtle,
            DcatProfile::eu(),
            &Default::default(),
        );
        dataroom_rdf::turtle::parse_turtle_bytes(ttl.as_bytes(), &RdfLimits::default())
            .unwrap_or_else(|e| panic!("a harvested dataset wrote unreadable Turtle: {e}\n{ttl}"));
    }
});
