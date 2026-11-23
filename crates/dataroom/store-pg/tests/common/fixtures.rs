//! Record builders for the `Store` contract, free of any store: the memory store's own tests
//! include this file and `contract.rs` by path, so the contract runs in both crates' tests.

use dataroom_core::{
    ActivityRow, Catalogue, CatalogueId, CataloguePlugin, Dataset, HarvestRun, HarvestSource,
    OrgId, ProfileRecord, Publisher, RequestCtx, ResourceView, ShaclValidationReport, StaticPage,
    Subject,
};
use jc_time::TimestampTz;
use serde_json::json;
use uuid::Uuid;

/// A fresh organization and catalogue.
pub fn tenant() -> (OrgId, CatalogueId) {
    (
        OrgId::from_uuid(Uuid::now_v7()),
        CatalogueId::from_uuid(Uuid::now_v7()),
    )
}

/// A host name no other test uses.
pub fn unique_host(prefix: &str) -> String {
    format!("{prefix}-{}.example.org", Uuid::now_v7().simple())
}

pub fn member_ctx(org: OrgId, cat: CatalogueId) -> RequestCtx {
    RequestCtx::new(
        org,
        cat,
        Subject::authenticated("editor", vec![org], Vec::new()),
    )
}

pub fn ts(s: &str) -> TimestampTz {
    TimestampTz(jc_time::parse_rfc3339(s).expect("timestamp"))
}

pub fn catalogue(org: OrgId, cat: CatalogueId, name: &str, host: &str) -> Catalogue {
    serde_json::from_value(json!({
        "orgId": org, "id": cat, "name": name, "host": host,
        "title": {"sk": format!("Katalóg {name}"), "en": format!("Catalogue {name}")},
        "description": {"sk": "Popis"},
        "defaultLanguage": "sk", "languages": ["sk", "en"],
        "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
        "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("catalogue fixture")
}

pub fn publisher(name: &str) -> Publisher {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(),
        "id": "", "name": name,
        "title": {"sk": format!("Vydavateľ {name}")}, "description": {},
        "uri": format!("https://example.org/publisher/{name}"),
        "state": "active", "editorGrants": [],
        "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("publisher fixture")
}

pub fn dataset(name: &str, publisher: &str, visibility: &str) -> Dataset {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(),
        "id": "", "name": name,
        "title": {"sk": format!("Dataset {name}"), "en": format!("Dataset {name} en")},
        "description": {"sk": "Popis dát o ovzduší"},
        "publisherId": publisher,
        "visibility": visibility, "state": "active",
        "tags": ["ovzdušie"],
        "distributions": [{
            "id": "", "datasetId": "",
            "title": {"sk": format!("CSV {name}")},
            "accessUrl": format!("https://example.org/{name}.csv"),
            "format": "CSV", "byteSize": 1234
        }],
        "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
    }))
    .expect("dataset fixture")
}

pub fn harvest_source(name: &str) -> HarvestSource {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(),
        "id": "", "name": name, "kind": "dcat-ap",
        "url": format!("https://harvest.example.org/{name}.ttl"),
        "publisherId": "city", "allowedHosts": ["harvest.example.org"],
        "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("harvest source fixture")
}

pub fn harvest_run(id: &str, source: &str, started: &str) -> HarvestRun {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(),
        "id": id, "sourceId": source, "state": "succeeded",
        "startedAt": started, "finishedAt": started,
        "createdCount": 2, "updatedCount": 1, "unchangedCount": 0, "withdrawnCount": 0
    }))
    .expect("harvest run fixture")
}

pub fn activity(id: &str, object: &str, at: &str) -> ActivityRow {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(),
        "id": id, "at": at, "actor": "editor", "operation": "package_update",
        "objectType": "dataset", "objectId": object, "summary": format!("updated {object}")
    }))
    .expect("activity fixture")
}

pub fn shacl_report(dataset: &str, profile: &str, conforms: bool) -> ShaclValidationReport {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(),
        "id": "", "datasetId": dataset, "profileId": profile, "profileVersion": "1",
        "conforms": conforms, "violationCount": if conforms { 0 } else { 2 }, "warningCount": 1,
        "findings": [], "checkedAt": "2026-11-01T10:00:00Z"
    }))
    .expect("shacl report fixture")
}

pub fn profile(id: &str, version: &str) -> ProfileRecord {
    serde_json::from_value(json!({
        "id": id, "version": version, "uri": format!("https://example.org/profile/{id}"),
        "title": {"en": id}, "shapesTurtle": "", "builtIn": false
    }))
    .expect("profile fixture")
}

pub fn page(slug: &str, lang: &str) -> StaticPage {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(),
        "slug": slug, "lang": lang, "title": format!("{slug} {lang}"),
        "markdown": "# Hello", "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("page fixture")
}

pub fn plugin(name: &str) -> CataloguePlugin {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(),
        "name": name, "version": "1.0.0", "digest": "sha256:00", "enabled": true,
        "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("plugin fixture")
}

pub fn view(id: &str, resource: &str, order: i32) -> ResourceView {
    serde_json::from_value(json!({
        "id": id, "resourceId": resource, "viewType": "jc_visualization",
        "title": format!("View {id}"), "order": order
    }))
    .expect("view fixture")
}
