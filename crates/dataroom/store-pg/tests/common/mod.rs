#![allow(dead_code)]
//! Shared fixtures for the store tests: a migrated Postgres database reached through a request
//! role without SUPERUSER or BYPASSRLS (the role production runs as, C1), and record builders.

use chrono::{DateTime, Utc};
use dataroom_core::{
    ActivityRow, Catalogue, CatalogueId, CataloguePlugin, Dataset, HarvestRun, HarvestSource,
    OrgId, ProfileRecord, Publisher, RequestCtx, ResourceView, ShaclValidationReport, StaticPage,
    Subject,
};
use dataroom_store_pg::PgStore;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::OnceCell;
use uuid::Uuid;

/// The request role the tests connect as (`SET ROLE` on every pooled connection).
pub const APP_ROLE: &str = "dataroom_app_test";

static MIGRATED: OnceCell<()> = OnceCell::const_new();

/// The test database URL, or `None` (printing `SKIP`) when it is unset. With
/// `DATAROOM_REQUIRE_PG=1` a missing URL fails the test instead of skipping it.
pub fn database_url(test: &str) -> Option<String> {
    match std::env::var("DATAROOM_TEST_DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(url) => Some(url),
        None => {
            assert!(
                std::env::var("DATAROOM_REQUIRE_PG").ok().as_deref() != Some("1"),
                "{test}: DATAROOM_REQUIRE_PG=1 but DATAROOM_TEST_DATABASE_URL is unset"
            );
            println!("SKIP {test}: DATAROOM_TEST_DATABASE_URL is unset");
            None
        }
    }
}

/// Pools for one test: `admin` is the migration owner (superuser), `app` the request role.
pub struct Pg {
    pub url: String,
    pub admin: PgPool,
    pub app: PgPool,
}

impl Pg {
    /// A store on the request role, checked by the boot check like `serve` does.
    pub async fn app_store(&self) -> PgStore {
        PgStore::connect_and_verify(self.app.clone())
            .await
            .expect("the request role passes the boot check")
    }
}

/// Migrates once per test binary, creates the request role, and opens both pools.
pub async fn pg(test: &str) -> Option<Pg> {
    let url = database_url(test)?;
    MIGRATED
        .get_or_init(|| async {
            let admin = PgPool::connect(&url).await.expect("connect admin");
            sqlx::query("SELECT pg_advisory_lock(hashtext('dataroom-test-setup'))")
                .execute(&admin)
                .await
                .expect("setup lock");
            PgStore::from_pool_unchecked(admin.clone())
                .migrate()
                .await
                .expect("migrate");
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DO $$
                 BEGIN
                     IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{APP_ROLE}') THEN
                         CREATE ROLE {APP_ROLE} NOLOGIN NOSUPERUSER NOBYPASSRLS;
                     END IF;
                 END $$;
                 GRANT USAGE ON SCHEMA dataroom TO {APP_ROLE};
                 GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA dataroom TO {APP_ROLE};"
            )))
            .execute(&admin)
            .await
            .expect("request role");
            sqlx::query("SELECT pg_advisory_unlock(hashtext('dataroom-test-setup'))")
                .execute(&admin)
                .await
                .expect("setup unlock");
            admin.close().await;
        })
        .await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect admin");
    let app = app_pool(&url, 2).await;
    Some(Pg { url, admin, app })
}

/// A pool whose every connection acts as [`APP_ROLE`].
pub async fn app_pool(url: &str, max: u32) -> PgPool {
    PgPoolOptions::new()
        .max_connections(max)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query(sqlx::AssertSqlSafe(format!("SET ROLE {APP_ROLE}")))
                    .execute(&mut *conn)
                    .await
                    .map(|_| ())
            })
        })
        .connect(url)
        .await
        .expect("connect as the request role")
}

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

pub fn ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .expect("timestamp")
        .with_timezone(&Utc)
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

pub mod contract;
