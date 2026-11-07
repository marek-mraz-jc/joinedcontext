#![forbid(unsafe_code)]
//! Harvest runner executing sources with SSRF protection, provenance tracking, and withdrawal
//! of datasets no longer listed by a complete source read (C9).

use crate::guard::{validate_outbound_url, MAX_HARVEST_BYTES};
use crate::sources::{
    parse_arcgis_hub_items, parse_ckan_search_page, parse_dcat_page, parse_nkod_sparql_bindings,
};
use chrono::Utc;
use dataroom_core::ops::HarvestRunner;
use dataroom_core::store::BoxFuture;
use dataroom_core::{
    ActivityRow, DataroomError, Dataset, HarvestLogLine, HarvestRun, HarvestRunState,
    HarvestSkippedItem, HarvestSource, HarvestSourceKind, RecordState, RequestCtx, Result, Store,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Outbound HTTP fetcher trait so both real HTTPS and deterministic test doubles go through
/// the exact same SSRF and host-allowlist checks.
pub trait OutboundFetcher: Send + Sync {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        accept: &'a str,
    ) -> BoxFuture<'a, Result<(Option<String>, String)>>;
}

/// Production HTTPS fetcher backed by `reqwest` with size cap and timeout.
pub struct HttpFetcher {
    client: reqwest::Client,
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpFetcher {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("dataroom-harvest/1.0")
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl OutboundFetcher for HttpFetcher {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        accept: &'a str,
    ) -> BoxFuture<'a, Result<(Option<String>, String)>> {
        Box::pin(async move {
            let resp = self
                .client
                .get(url)
                .header("Accept", accept)
                .send()
                .await
                .map_err(|e| DataroomError::OutboundRefused(format!("fetch {url}: {e}")))?;
            if !resp.status().is_success() {
                return Err(DataroomError::OutboundRefused(format!(
                    "{url} answered HTTP {}",
                    resp.status().as_u16()
                )));
            }
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| DataroomError::OutboundRefused(format!("read {url}: {e}")))?;
            if bytes.len() > MAX_HARVEST_BYTES {
                return Err(DataroomError::OutboundRefused(format!(
                    "{url} answered more than {MAX_HARVEST_BYTES} bytes"
                )));
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            Ok((content_type, text))
        })
    }
}

/// Deterministic in-memory map fetcher for testing harvest runs, pagination and withdrawal.
#[derive(Clone, Default)]
pub struct MapFetcher {
    responses: Arc<std::sync::RwLock<BTreeMap<String, (Option<String>, String)>>>,
}

impl MapFetcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, url: impl Into<String>, content_type: Option<&str>, body: impl Into<String>) {
        self.responses
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(url.into(), (content_type.map(str::to_owned), body.into()));
    }
}

impl OutboundFetcher for MapFetcher {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        _accept: &'a str,
    ) -> BoxFuture<'a, Result<(Option<String>, String)>> {
        let found = self
            .responses
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(url)
            .cloned()
            .ok_or_else(|| DataroomError::OutboundRefused(format!("no response for {url}")));
        Box::pin(std::future::ready(found))
    }
}

/// Harvest runner engine implementing [`HarvestRunner`] (C9).
pub struct HarvestEngine {
    fetcher: Arc<dyn OutboundFetcher>,
    resolve_dns: bool,
}

impl HarvestEngine {
    pub fn new(fetcher: Arc<dyn OutboundFetcher>) -> Self {
        Self {
            fetcher,
            resolve_dns: true,
        }
    }

    pub fn with_dns_resolution(mut self, resolve_dns: bool) -> Self {
        self.resolve_dns = resolve_dns;
        self
    }

    async fn checked_fetch(
        &self,
        url: &str,
        source: &HarvestSource,
        cat_allowed_hosts: &[String],
        accept: &str,
    ) -> Result<(Option<String>, String)> {
        validate_outbound_url(
            url,
            &source.allowed_hosts,
            cat_allowed_hosts,
            self.resolve_dns,
        )?;
        self.fetcher.fetch(url, accept).await
    }

    async fn gather_items(
        &self,
        ctx: &RequestCtx,
        store: &dyn Store,
        source: &HarvestSource,
        cat_allowed_hosts: &[String],
        default_lang: &str,
    ) -> Result<(Vec<(String, Dataset)>, Vec<HarvestSkippedItem>)> {
        // Check cross-organization harvest policy if the source URL points to another catalogue
        // on this installation (C9).
        if let Ok(parsed) = url::Url::parse(&source.url) {
            if let Some(host) = parsed.host_str() {
                if let Some(target_cat) = store.resolve_host(host).await? {
                    if target_cat.org_id != ctx.org_id && !target_cat.allow_cross_org_harvest {
                        return Err(DataroomError::Forbidden(format!(
                            "catalogue '{}' does not allow cross-organization harvesting",
                            target_cat.host
                        )));
                    }
                }
            }
        }

        match source.kind {
            HarvestSourceKind::ArcGisHub => {
                let mut features = Vec::new();
                let hub = source.url.trim_end_matches('/');
                let mut next_url = Some(format!(
                    "{hub}/api/search/v1/collections/dataset/items?limit=100"
                ));
                let mut expected_total = None;

                for _ in 0..50 {
                    let Some(url) = next_url.take() else {
                        break;
                    };
                    let (_, body) = self
                        .checked_fetch(&url, source, cat_allowed_hosts, "application/json")
                        .await?;
                    let page: Value = serde_json::from_str(&body).map_err(|e| {
                        DataroomError::OutboundRefused(format!("invalid Hub JSON: {e}"))
                    })?;
                    if let Some(m) = page.get("numberMatched").and_then(Value::as_u64) {
                        expected_total = Some(m as usize);
                    }
                    if let Some(arr) = page.get("features").and_then(Value::as_array) {
                        features.extend(arr.iter().cloned());
                    }
                    next_url = page
                        .get("links")
                        .and_then(Value::as_array)
                        .and_then(|links| {
                            links.iter().find_map(|l| {
                                (l.get("rel").and_then(Value::as_str) == Some("next"))
                                    .then(|| l.get("href").and_then(Value::as_str).map(str::to_owned))
                                    .flatten()
                            })
                        });
                }

                if let Some(expected) = expected_total {
                    if features.len() != expected {
                        return Err(DataroomError::OutboundRefused(format!(
                            "incomplete Hub read: got {} of {expected} items",
                            features.len()
                        )));
                    }
                }

                Ok(parse_arcgis_hub_items(
                    ctx.org_id,
                    ctx.catalogue_id,
                    source,
                    cat_allowed_hosts,
                    &features,
                    default_lang,
                ))
            }
            HarvestSourceKind::NkodSparql => {
                let (_, body) = self
                    .checked_fetch(
                        &source.url,
                        source,
                        cat_allowed_hosts,
                        "application/sparql-results+json",
                    )
                    .await?;
                let val: Value = serde_json::from_str(&body).map_err(|e| {
                    DataroomError::OutboundRefused(format!("invalid SPARQL JSON: {e}"))
                })?;
                Ok(parse_nkod_sparql_bindings(
                    ctx.org_id,
                    ctx.catalogue_id,
                    source,
                    cat_allowed_hosts,
                    &val,
                    default_lang,
                ))
            }
            HarvestSourceKind::DcatAp => {
                let mut all_datasets = Vec::new();
                let mut next_url = Some(source.url.clone());
                let mut seen_pages = BTreeSet::new();

                for _ in 0..50 {
                    let Some(url) = next_url.take() else {
                        break;
                    };
                    if !seen_pages.insert(url.clone()) {
                        break;
                    }
                    let (ct, body) = self
                        .checked_fetch(
                            &url,
                            source,
                            cat_allowed_hosts,
                            "text/turtle, application/ld+json, application/rdf+xml",
                        )
                        .await?;
                    let (page_ds, next) = parse_dcat_page(
                        ctx.org_id,
                        ctx.catalogue_id,
                        source,
                        &body,
                        ct.as_deref(),
                        default_lang,
                    )?;
                    all_datasets.extend(page_ds);
                    next_url = next;
                }

                Ok((all_datasets, Vec::new()))
            }
            HarvestSourceKind::CkanAction => {
                let mut all_datasets = Vec::new();
                let mut start = 0usize;
                let base = source.url.trim_end_matches('/');
                for _ in 0..20 {
                    let url = if base.contains("/api/3/action/package_search") {
                        format!("{base}?rows=1000&start={start}")
                    } else {
                        format!("{base}/api/3/action/package_search?rows=1000&start={start}")
                    };
                    let (_, body) = self
                        .checked_fetch(&url, source, cat_allowed_hosts, "application/json")
                        .await?;
                    let (page_ds, total) = parse_ckan_search_page(
                        ctx.org_id,
                        ctx.catalogue_id,
                        source,
                        &body,
                        default_lang,
                    )?;
                    let fetched_len = page_ds.len();
                    all_datasets.extend(page_ds);
                    start += fetched_len;
                    if fetched_len == 0 || start >= total {
                        break;
                    }
                }
                Ok((all_datasets, Vec::new()))
            }
        }
    }
}

impl HarvestRunner for HarvestEngine {
    fn run_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        store: &'a dyn Store,
        source: &'a HarvestSource,
    ) -> BoxFuture<'a, Result<HarvestRun>> {
        Box::pin(async move {
            let started_at = Utc::now();
            let run_id = format!("run-{}", Uuid::now_v7());
            let mut logs = Vec::new();

            let cat = store
                .get_catalogue(ctx.org_id, ctx.catalogue_id)
                .await?
                .ok_or_else(|| DataroomError::NotFound("catalogue not found".into()))?;

            // Ensure target publisher exists (`harvest-hub.py` line 213).
            if store.get_publisher(ctx, &source.publisher_id).await?.is_none() {
                return Err(DataroomError::Validation {
                    message: format!("publisher '{}' does not exist", source.publisher_id),
                    fields: BTreeMap::new(),
                });
            }

            let gather_res = self
                .gather_items(
                    ctx,
                    store,
                    source,
                    &cat.allowed_outbound_hosts,
                    &cat.default_language,
                )
                .await;

            let (kept_items, skipped) = match gather_res {
                Ok(pair) => pair,
                Err(err) => {
                    logs.push(HarvestLogLine {
                        at: Utc::now(),
                        level: "error".into(),
                        fingerprint: format!("harvest.{}.fetch_error", source.kind.as_str()),
                        message: err.to_string(),
                    });
                    let failed_run = HarvestRun {
                        org_id: ctx.org_id,
                        catalogue_id: ctx.catalogue_id,
                        id: run_id,
                        source_id: source.id.clone(),
                        state: HarvestRunState::Failed,
                        started_at,
                        finished_at: Some(Utc::now()),
                        created_count: 0,
                        updated_count: 0,
                        unchanged_count: 0,
                        withdrawn_count: 0,
                        skipped: Vec::new(),
                        error: Some(err.to_string()),
                        logs,
                    };
                    store.upsert_harvest_run(ctx, failed_run).await?;
                    return Err(err);
                }
            };

            if kept_items.is_empty() {
                let err = DataroomError::OutboundRefused(
                    "source listed 0 items to keep; nothing is written or withdrawn".into(),
                );
                return Err(err);
            }

            // Index existing datasets from this harvest source by `harvest_source_item_id`.
            let existing_all = store.list_datasets(ctx).await?;
            let mut existing_by_item: BTreeMap<String, Dataset> = BTreeMap::new();
            for ds in existing_all {
                let hs = ds
                    .harvest_source_id
                    .as_deref()
                    .or_else(|| ds.extras.get("harvest_source").map(String::as_str));
                if hs == Some(source.name.as_str()) || hs == Some(source.id.as_str()) {
                    let item_id = ds
                        .harvest_source_item_id
                        .clone()
                        .or_else(|| ds.extras.get("source_id").cloned())
                        .or_else(|| ds.extras.get("source_uri").cloned())
                        .unwrap_or_else(|| ds.name.clone());
                    existing_by_item.insert(item_id, ds);
                }
            }

            let mut created_count = 0usize;
            let mut updated_count = 0usize;
            let mut unchanged_count = 0usize;
            let mut withdrawn_count = 0usize;

            for (item_id, mut incoming) in kept_items {
                incoming.harvest_source_id = Some(source.name.clone());
                incoming.harvest_source_item_id = Some(item_id.clone());
                if let Some(old) = existing_by_item.remove(&item_id) {
                    incoming.id = old.id.clone();
                    incoming.name = old.name.clone();
                    if old.state == RecordState::Active
                        && old.harvest_hash.is_some()
                        && old.harvest_hash == incoming.harvest_hash
                    {
                        unchanged_count += 1;
                    } else {
                        incoming.state = RecordState::Active;
                        store.upsert_dataset(ctx, incoming).await?;
                        updated_count += 1;
                    }
                } else {
                    store.upsert_dataset(ctx, incoming).await?;
                    created_count += 1;
                }
            }

            // Withdraw any previously harvested dataset from this source that was not in the complete read.
            for (gone_item_id, gone_ds) in existing_by_item {
                if gone_ds.state == RecordState::Active {
                    store.withdraw_dataset(ctx, &gone_ds.id).await?;
                    withdrawn_count += 1;
                    logs.push(HarvestLogLine {
                        at: Utc::now(),
                        level: "info".into(),
                        fingerprint: "harvest.withdrawn".into(),
                        message: format!(
                            "Withdrew dataset '{}' (source item {})",
                            gone_ds.name, gone_item_id
                        ),
                    });
                }
            }

            for sk in &skipped {
                logs.push(HarvestLogLine {
                    at: Utc::now(),
                    level: "info".into(),
                    fingerprint: "harvest.skipped".into(),
                    message: format!("Skipped {} {:?}: {}", sk.item_id, sk.title, sk.reason),
                });
            }

            let run = HarvestRun {
                org_id: ctx.org_id,
                catalogue_id: ctx.catalogue_id,
                id: run_id,
                source_id: source.id.clone(),
                state: HarvestRunState::Succeeded,
                started_at,
                finished_at: Some(Utc::now()),
                created_count,
                updated_count,
                unchanged_count,
                withdrawn_count,
                skipped,
                error: None,
                logs,
            };
            let saved_run = store.upsert_harvest_run(ctx, run).await?;
            store
                .record_activity(
                    ctx,
                    ActivityRow {
                        org_id: ctx.org_id,
                        catalogue_id: ctx.catalogue_id,
                        id: format!("act-{}", Uuid::now_v7()),
                        at: Utc::now(),
                        actor: ctx.actor_name().to_owned(),
                        operation: "harvest.run".into(),
                        object_type: "HarvestSource".into(),
                        object_id: source.id.clone(),
                        summary: format!(
                            "Harvest {}: created {}, updated {}, unchanged {}, withdrawn {}",
                            source.name,
                            created_count,
                            updated_count,
                            unchanged_count,
                            withdrawn_count
                        ),
                        details: json!({
                            "run_id": saved_run.id,
                            "created": created_count,
                            "updated": updated_count,
                            "unchanged": unchanged_count,
                            "withdrawn": withdrawn_count,
                        }),
                    },
                )
                .await?;
            Ok(saved_run)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dataroom_core::{Catalogue, CatalogueId, OrgId, Publisher, Subject, ThemeTokens};
    use dataroom_store_memory::MemoryStore;

    fn test_setup() -> (OrgId, CatalogueId, RequestCtx, MemoryStore) {
        let org_id = OrgId::from_uuid(Uuid::from_u128(1));
        let catalogue_id = CatalogueId::from_uuid(Uuid::from_u128(10));
        let ctx = RequestCtx::new(
            org_id,
            catalogue_id,
            Subject::authenticated("harvester", vec![org_id], Vec::new()),
        );
        let store = MemoryStore::new();
        (org_id, catalogue_id, ctx, store)
    }

    #[test]
    fn ssrf_guard_refuses_private_ips_plain_http_and_unlisted_hosts() {
        let allowed = vec!["opendata.bbsk.sk".to_owned()];
        // 1. Plain http refused
        assert!(validate_outbound_url(
            "http://opendata.bbsk.sk/api/items",
            &allowed,
            &[],
            false
        )
        .is_err());

        // 2. Private IPv4 ranges, loopback, and cloud metadata refused even if added to allowlist
        for internal in [
            "https://127.0.0.1/api/3/action/package_search",
            "https://10.0.0.1/catalog.ttl",
            "https://172.16.5.4/catalog.ttl",
            "https://192.168.1.1/catalog.ttl",
            "https://169.254.169.254/latest/meta-data/",
            "https://100.64.0.1/catalog.ttl",
            "https://[::1]/catalog.ttl",
            "https://localhost/catalog.ttl",
            "https://ckan.default.svc.cluster.local/api",
        ] {
            let host = url::Url::parse(internal)
                .unwrap()
                .host_str()
                .unwrap()
                .to_owned();
            assert!(
                validate_outbound_url(internal, &[host], &[], false).is_err(),
                "{internal} must be refused by SSRF guard"
            );
        }

        // 3. Host outside allowlist refused
        assert!(validate_outbound_url(
            "https://evil.example.org/catalog.ttl",
            &allowed,
            &[],
            false
        )
        .is_err());

        // 4. URL with embedded credentials refused
        assert!(validate_outbound_url(
            "https://user:secret@opendata.bbsk.sk/api",
            &allowed,
            &[],
            false
        )
        .is_err());

        // 5. Allowed public https URL passes
        assert!(validate_outbound_url(
            "https://opendata.bbsk.sk/api/search/v1/collections/dataset/items?limit=100",
            &allowed,
            &[],
            false
        )
        .is_ok());
    }

    #[tokio::test]
    async fn harvest_creates_deduplicates_keeps_unchanged_and_withdraws_dropped_items() {
        let (org_id, catalogue_id, ctx, store) = test_setup();
        store
            .upsert_catalogue(Catalogue {
                org_id,
                id: catalogue_id,
                name: "bbsk".into(),
                host: "opendata.bbsk.sk".into(),
                title: BTreeMap::from([("sk".into(), "BBSK".into())]),
                description: BTreeMap::from([("sk".into(), "Katalóg".into())]),
                homepage: None,
                default_language: "sk".into(),
                languages: vec!["sk".into(), "en".into()],
                main_profile: "dcat-ap-sk-2.0".into(),
                active_profiles: Vec::new(),
                default_licence: Some("cc-by".into()),
                publisher_uri: Some("https://data.gov.sk/id/legal-subject/37828100".into()),
                publisher_name: None,
                contact_point: None,
                theme: ThemeTokens::default(),
                allow_cross_org_harvest: true,
                allowed_outbound_hosts: vec!["hub.bbsk.sk".into()],
                applicable_legislation: Vec::new(),
                modified: Utc::now(),
            })
            .await
            .unwrap();

        store
            .upsert_publisher(
                &ctx,
                Publisher {
                    org_id,
                    catalogue_id,
                    id: "pub-bbsk".into(),
                    name: "bbsk".into(),
                    title: BTreeMap::from([("sk".into(), "BBSK".into())]),
                    description: BTreeMap::new(),
                    uri: "https://data.gov.sk/id/legal-subject/37828100".into(),
                    publisher_type: None,
                    identifier: None,
                    email: None,
                    homepage: None,
                    logo_url: None,
                    contact_point: None,
                    state: RecordState::Active,
                    merged_into: None,
                    editor_grants: Vec::new(),
                    created: Utc::now(),
                    modified: Utc::now(),
                },
            )
            .await
            .unwrap();

        let source = HarvestSource {
            org_id,
            catalogue_id,
            id: "hs-hub".into(),
            name: "hub-bbsk".into(),
            kind: HarvestSourceKind::ArcGisHub,
            url: "https://hub.bbsk.sk".into(),
            publisher_id: "bbsk".into(),
            target_publisher_iri: Some("https://data.gov.sk/id/legal-subject/37828100".into()),
            spatial_uri: Some("http://data.europa.eu/nuts/code/SK032".into()),
            name_prefix: String::new(),
            allowed_hosts: vec!["hub.bbsk.sk".into()],
            rewrite_hosts: BTreeMap::new(),
            skip_items: BTreeMap::new(),
            title_overrides: BTreeMap::new(),
            spelling_fixes: BTreeMap::new(),
            require_column: BTreeMap::new(),
            schedule: "23 3 * * *".into(),
            enabled: true,
            modified: Utc::now(),
        };
        store.upsert_harvest_source(&ctx, source.clone()).await.unwrap();

        let fetcher = MapFetcher::new();
        let hub_url = "https://hub.bbsk.sk/api/search/v1/collections/dataset/items?limit=100";

        // Run 1: 2 valid open datasets + 1 closed-licence item (skipped)
        fetcher.insert(
            hub_url,
            Some("application/json"),
            json!({
                "numberMatched": 3,
                "features": [
                    {
                        "properties": {
                            "id": "item111111",
                            "title": "Cesty II. a III. triedy",
                            "type": "Feature Service",
                            "license": "CC-BY-4.0",
                            "url": "https://hub.bbsk.sk/arcgis/rest/services/cesty/FeatureServer",
                            "description": "<p>Správa ciest</p>",
                            "created": 1700000000000i64,
                            "modified": 1700000000000i64,
                            "tags": ["doprava", "cesty"]
                        }
                    },
                    {
                        "properties": {
                            "id": "item222222",
                            "title": "Stredné školy",
                            "type": "Shapefile",
                            "license": "CC0-1.0",
                            "description": "Zoznam škôl",
                            "created": 1700000000000i64,
                            "modified": 1700000000000i64,
                            "tags": ["skoly"]
                        }
                    },
                    {
                        "properties": {
                            "id": "item333333",
                            "title": "Interná mapa",
                            "type": "Shapefile",
                            "license": "Proprietary"
                        }
                    }
                ],
                "links": []
            })
            .to_string(),
        );

        let engine = HarvestEngine::new(Arc::new(fetcher.clone())).with_dns_resolution(false);
        let run1 = engine.run_source(&ctx, &store, &source).await.expect("run 1");
        assert_eq!(run1.created_count, 2);
        assert_eq!(run1.updated_count, 0);
        assert_eq!(run1.unchanged_count, 0);
        assert_eq!(run1.withdrawn_count, 0);
        assert_eq!(run1.skipped.len(), 1);

        // Run 2: item222222 is dropped by the source; item111111 is unchanged -> item222222 is withdrawn!
        fetcher.insert(
            hub_url,
            Some("application/json"),
            json!({
                "numberMatched": 1,
                "features": [
                    {
                        "properties": {
                            "id": "item111111",
                            "title": "Cesty II. a III. triedy",
                            "type": "Feature Service",
                            "license": "CC-BY-4.0",
                            "url": "https://hub.bbsk.sk/arcgis/rest/services/cesty/FeatureServer",
                            "description": "<p>Správa ciest</p>",
                            "created": 1700000000000i64,
                            "modified": 1700000000000i64,
                            "tags": ["doprava", "cesty"]
                        }
                    }
                ],
                "links": []
            })
            .to_string(),
        );

        let run2 = engine.run_source(&ctx, &store, &source).await.expect("run 2");
        assert_eq!(run2.created_count, 0);
        assert_eq!(run2.unchanged_count, 1);
        assert_eq!(run2.withdrawn_count, 1);

        let all_ds = store.list_datasets(&ctx).await.unwrap();
        let active: Vec<&str> = all_ds
            .iter()
            .filter(|d| d.state == RecordState::Active)
            .map(|d| d.name.as_str())
            .collect();
        let withdrawn: Vec<&str> = all_ds
            .iter()
            .filter(|d| d.state == RecordState::Withdrawn)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(withdrawn.len(), 1);
    }
}
