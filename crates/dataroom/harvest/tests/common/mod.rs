#![allow(dead_code)]
//! Fixtures for the harvest tests: a catalogue with one publisher in a memory store, sources of
//! every kind, and an engine that fetches from a [`MapFetcher`] with a fixed public DNS answer.

use dataroom_core::{
    Catalogue, CatalogueId, Dataset, HarvestSource, OrgId, Publisher, RequestCtx, Store, Subject,
};
use dataroom_harvest::{HarvestEngine, MapFetcher};
use dataroom_store_memory::MemoryStore;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use uuid::Uuid;

pub struct World {
    pub org: OrgId,
    pub cat: CatalogueId,
    pub ctx: RequestCtx,
    pub store: MemoryStore,
}

pub fn catalogue(org: OrgId, cat: CatalogueId, host: &str, outbound: &[&str]) -> Catalogue {
    serde_json::from_value(json!({
        "orgId": org, "id": cat, "name": "main", "host": host,
        "title": {"sk": "Katalóg"}, "description": {},
        "defaultLanguage": "sk", "languages": ["sk"],
        "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
        "allowedOutboundHosts": outbound,
        "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("catalogue")
}

pub fn publisher(name: &str) -> Publisher {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "", "name": name,
        "title": {"sk": name}, "description": {}, "uri": format!("https://example.org/{name}"),
        "state": "active", "editorGrants": [],
        "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("publisher")
}

/// A catalogue on `opendata.city.example` that may reach the given outbound hosts, with the
/// publisher `city`.
pub async fn world(outbound: &[&str]) -> World {
    let org = OrgId::from_uuid(Uuid::now_v7());
    let cat = CatalogueId::from_uuid(Uuid::now_v7());
    let ctx = RequestCtx::new(
        org,
        cat,
        Subject::authenticated("harvester", vec![org], Vec::new()),
    );
    let store = MemoryStore::new();
    store
        .upsert_catalogue(catalogue(org, cat, "opendata.city.example", outbound))
        .await
        .unwrap();
    store
        .upsert_publisher(&ctx, publisher("city"))
        .await
        .unwrap();
    World {
        org,
        cat,
        ctx,
        store,
    }
}

pub fn source(kind: &str, name: &str, url: &str, hosts: &[&str]) -> HarvestSource {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": format!("hs-{name}"), "name": name,
        "kind": kind, "url": url, "publisherId": "city", "allowedHosts": hosts,
        "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("source")
}

/// An engine on `fetcher` where every name resolves to one public address.
pub fn engine(fetcher: &MapFetcher) -> HarvestEngine {
    HarvestEngine::new(Arc::new(fetcher.clone()))
        .with_custom_dns(|_, port| Ok(vec![SocketAddr::from(([93, 184, 216, 34], port))]))
}

/// A local dataset (not harvested) in the world's catalogue.
pub fn local_dataset(id: &str, name: &str, title: &str) -> Dataset {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": id, "name": name,
        "title": {"sk": title}, "description": {"sk": "Ručne spravovaný dataset"},
        "publisherId": "city", "visibility": "public", "state": "active",
        "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("dataset")
}

pub fn hub_item(id: &str, title: &str, modified: i64) -> Value {
    json!({"properties": {
        "id": id, "title": title, "type": "Shapefile", "license": "CC-BY-4.0",
        "description": format!("<p>{title}</p>"), "created": 1_700_000_000_000i64,
        "modified": modified, "tags": ["doprava"]
    }})
}

pub fn ckan_package(id: &str, name: &str, title: &str) -> Value {
    json!({
        "id": id, "name": name, "title": title, "notes": format!("Popis {title}"),
        "license_id": "cc-by", "tags": [{"name": "ovzdusie"}],
        "resources": [{"id": format!("{id}-r"), "name": "CSV",
                       "url": format!("https://ckan.remote.example/{name}.csv"), "format": "CSV"}]
    })
}

pub fn ckan_page(count: usize, results: Vec<Value>) -> String {
    json!({"success": true, "result": {"count": count, "results": results}}).to_string()
}

pub fn dcat_turtle(datasets: &[(&str, &str)], next: Option<&str>) -> String {
    let mut ttl = String::from(
        "@prefix dcat: <http://www.w3.org/ns/dcat#> .\n\
         @prefix dct: <http://purl.org/dc/terms/> .\n\
         @prefix hydra: <http://www.w3.org/ns/hydra/core#> .\n",
    );
    for (iri, title) in datasets {
        ttl.push_str(&format!(
            "<{iri}> a dcat:Dataset ; dct:title \"{title}\"@sk ; dct:description \"Popis {title}\"@sk ;\n\
             dcat:distribution [ a dcat:Distribution ; dcat:accessURL <{iri}.csv> ] .\n"
        ));
    }
    if let Some(n) = next {
        ttl.push_str(&format!(
            "<https://feeds.example.org/catalog> hydra:next <{n}> .\n"
        ));
    }
    ttl
}

pub async fn active_names(w: &World) -> Vec<String> {
    let mut v: Vec<String> = w
        .store
        .list_datasets(&w.ctx)
        .await
        .unwrap()
        .into_iter()
        .filter(|d| d.state == dataroom_core::RecordState::Active)
        .map(|d| d.name)
        .collect();
    v.sort();
    v
}
