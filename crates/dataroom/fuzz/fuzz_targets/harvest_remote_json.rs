//! Input: a remote catalogue's JSON answer, as a harvest run reads it from a CKAN Action API,
//! an ArcGIS Hub, an ArcGIS REST directory or service, an OGC API Records collection or the NKOD
//! SPARQL endpoint. Each reader keeps or skips records and never panics; every kept record has
//! an item id, and a CKAN page never yields more records than its body has bytes.
#![no_main]

use dataroom_core::{CatalogueId, HarvestSource, OrgId};
use dataroom_harvest::sources::{
    parse_arcgis_directory, parse_arcgis_hub_items, parse_arcgis_service,
    parse_ckan_search_page, parse_nkod_sparql_bindings, parse_ogc_records_page,
};
use libfuzzer_sys::fuzz_target;
use serde_json::{json, Value};
use uuid::Uuid;

fn source(kind: &str, url: &str) -> HarvestSource {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "hs-f", "name": "f",
        "kind": kind, "url": url, "publisherId": "city",
        "allowedHosts": ["remote.example", "www.remote.example"],
        "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("source")
}

fuzz_target!(|data: &[u8]| {
    let Ok(body) = std::str::from_utf8(data) else {
        return;
    };
    let org = OrgId::from_uuid(Uuid::from_u128(1));
    let cat = CatalogueId::from_uuid(Uuid::from_u128(10));
    let allowed = ["remote.example".to_owned()];

    if let Ok((kept, _)) = parse_ckan_search_page(
        org,
        cat,
        &source("ckan-action", "https://remote.example/api/3"),
        body,
        "sk",
    ) {
        assert!(kept.len() <= body.len());
    }
    let _ = parse_arcgis_directory(body);
    let _ = parse_arcgis_service(
        org,
        cat,
        &source("arcgis-rest", "https://remote.example/arcgis/rest/services"),
        "https://remote.example/arcgis/rest/services/A/MapServer",
        "A/MapServer",
        body,
        "sk",
    );
    if let Ok(page) = parse_ogc_records_page(
        org,
        cat,
        &source("ogc-records", "https://remote.example/collections/c"),
        "https://remote.example/collections/c/items?f=json",
        body,
        "sk",
    ) {
        assert!(page.kept.iter().all(|(id, _)| !id.is_empty()));
    }
    let Ok(val) = serde_json::from_str::<Value>(body) else {
        return;
    };
    let (kept, _) = parse_nkod_sparql_bindings(
        org,
        cat,
        &source("nkod-sparql", "https://remote.example/sparql"),
        &allowed,
        &val,
        "cs",
    );
    assert!(kept.iter().all(|(id, _)| !id.is_empty()));
    let features = val
        .get("features")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let _ = parse_arcgis_hub_items(
        org,
        cat,
        &source("arc-gis-hub", "https://remote.example"),
        &allowed,
        &features,
        "sk",
    );
});
