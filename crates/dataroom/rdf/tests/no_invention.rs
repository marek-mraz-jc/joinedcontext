//! A feed states what the record declares and nothing else (CMP-08, SEC-17).

use std::collections::BTreeMap;

use dataroom_core::{Catalogue, Dataset};
use dataroom_rdf::{
    parse_rdf_graph, serialize_catalogue, serialize_dataset, DcatProfile, RdfFormat, RdfLimits,
};
use serde_json::{json, Value};

/// A catalogue and a dataset declaring a title and one address: no publisher, terms, licence,
/// theme, frequency or checksum algorithm.
fn bare() -> (Catalogue, Dataset) {
    let (org, id) = (uuid::Uuid::from_u128(1), uuid::Uuid::from_u128(2));
    let catalogue: Value = json!({
        "orgId": org, "id": id, "name": "opendata", "host": "opendata.example.org",
        "title": {"en": "Catalogue"}, "description": {}, "defaultLanguage": "en", "languages": ["en"],
        "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [], "modified": "2026-11-01T10:00:00Z"
    });
    let dataset: Value = json!({
        "orgId": org, "catalogueId": id, "id": "pkg-bare", "name": "bare",
        "title": {"en": "Bare"}, "description": {},
        "publisherId": "undeclared-office", "visibility": "public", "state": "active",
        "distributions": [{
            "id": "res-1", "datasetId": "pkg-bare", "title": {},
            "accessUrl": "https://files.example.org/bare", "hash": "00ff"
        }],
        "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
    });
    (
        serde_json::from_value(catalogue).expect("catalogue"),
        serde_json::from_value(dataset).expect("dataset"),
    )
}

/// What a bare record must never be rendered with, whatever the profile.
const UNDECLARED: &[(&str, &str)] = &[
    ("personal data", "personal-data-occurence-type"),
    ("personal data", "osobní-údaje"),
    ("personal data", "osobné_údaje"),
    ("personal data", "osobní_údaje"),
    ("database protection", "chráněn"),
    ("database protection", "databáza_"),
    ("licence", "creativecommons.org/licenses"),
    ("licence", "creativecommons.org/publicdomain"),
    ("licence", "neobsahuje-autorská-díla"),
    ("placeholder publisher", "/00000000"),
    ("minted publisher", "/publisher/undeclared-office"),
    ("minted publisher", "/publisher/default"),
    ("publisher name", "Open Data"),
    ("theme", "data-theme/GOVE"),
    ("frequency", "frequency/IRREG"),
    ("checksum algorithm", "checksumAlgorithm_sha256"),
];

fn inventions(profile: DcatProfile) -> Vec<String> {
    let (catalogue, dataset) = bare();
    let mut found = Vec::new();
    for format in [
        RdfFormat::Turtle,
        RdfFormat::JsonLd,
        RdfFormat::RdfXml,
        RdfFormat::NTriples,
    ] {
        let one = serialize_dataset(&dataset, &catalogue, None, format, profile);
        let all = serialize_catalogue(
            &catalogue,
            std::slice::from_ref(&dataset),
            &BTreeMap::new(),
            format,
            profile,
            None,
        );
        for text in [one, all] {
            // A context dictionary names every term; the statements are what follows it.
            let stated = parse_rdf_graph(&text, format, &RdfLimits::default())
                .map(|g| format!("{g:?}\n{text}"))
                .unwrap_or(text);
            for (what, value) in UNDECLARED {
                if stated.contains(value) {
                    found.push(format!("{format:?}: {what} ({value})"));
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn eu_renders_no_undeclared_value() {
    let found = inventions(DcatProfile::eu());
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn cz_renders_no_undeclared_value() {
    let found = inventions(DcatProfile::from_token("dcat-ap-cz").expect("installed"));
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn sk_renders_no_undeclared_value() {
    let found = inventions(DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"));
    assert!(found.is_empty(), "{found:#?}");
}
