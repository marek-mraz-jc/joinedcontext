#![forbid(unsafe_code)]
//! CLI and server entrypoints for `dataroom`:
//! - `serve`: starts the HTTP server (backed by Postgres with RLS verification or in-memory store).
//! - `migrate`: runs forward-only SQL migrations in schema `dataroom` and verifies forced RLS.
//! - `harvest`: runs a configured harvest source.
//! - `feed-check`: validates all public datasets and the catalogue feed against active SHACL profiles.
//! - `import-ckan`: imports datasets and organizations from a CKAN Action API JSON dump or URL.

use chrono::Utc;
use clap::{Parser, Subcommand};
use dataroom_api::{router, DataroomState};
use dataroom_core::ckan_compat::dataset_from_ckan;
use dataroom_core::ops::{execute_operation, DatasetValidator, HarvestRunner};
use dataroom_core::{
    Catalogue, CatalogueId, DataroomError, OrgId, PolicyAuthorizer, ProfilePin, Publisher,
    RecordState, RequestCtx, Result, Store, Subject, ThemeTokens,
};
use dataroom_harvest::{HarvestEngine, HttpFetcher};
use dataroom_shacl::ProfileRegistry;
use dataroom_store_memory::MemoryStore;
use dataroom_store_pg::PgStore;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Command-line interface for the `dataroom` binary.
#[derive(Debug, Parser)]
#[command(
    name = "dataroom",
    version,
    about = "Rust open and internal dataset catalogue"
)]
pub struct Cli {
    /// PostgreSQL connection URL; when absent, uses the in-memory store for standalone/demo runs.
    #[arg(long, env = "DATAROOM_DATABASE_URL")]
    pub database_url: Option<String>,

    /// Organization UUID for CLI operations.
    #[arg(
        long,
        env = "DATAROOM_ORG_ID",
        default_value = "00000000-0000-7000-8000-000000000001"
    )]
    pub org_id: String,

    /// Catalogue UUID for CLI operations.
    #[arg(
        long,
        env = "DATAROOM_CATALOGUE_ID",
        default_value = "00000000-0000-7000-8000-000000000010"
    )]
    pub catalogue_id: String,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the dataroom HTTP server (REST, OData, CKAN Action API v3, DCAT feeds, MCP, HTML UI).
    Serve {
        #[arg(long, env = "DATAROOM_LISTEN", default_value = "0.0.0.0:5000")]
        listen: String,
        #[arg(long, env = "DATAROOM_HOST", default_value = "opendata.localhost")]
        host: String,
    },
    /// Apply PostgreSQL schema migrations and verify forced Row-Level Security (C1).
    Migrate,
    /// Run a harvest source by name or ID (C9).
    Harvest {
        #[arg(long)]
        source: String,
    },
    /// Validate a catalogue's public DCAT-AP feed against its SHACL shapes (C8, C17).
    FeedCheck {
        #[arg(long)]
        profile: Option<String>,
    },
    /// Import organizations and datasets from a CKAN JSON export file (`package_search` / dump).
    ImportCkan {
        #[arg(long)]
        file: PathBuf,
    },
    /// Invoke any operation from the shared operation registry (`C11`).
    Op {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "{}")]
        input: String,
    },
}

/// Creates the default catalogue `catalogue_id` of `org_id` on `host` when the store has none yet,
/// as `serve` does at start. Used by the standalone binary and by `jc`, which hosts dataroom in its
/// own process (in-memory store by default).
pub async fn ensure_default_catalogue(
    store: &dyn Store,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    host: &str,
) -> Result<()> {
    if store.get_catalogue(org_id, catalogue_id).await?.is_none() {
        let host = host.to_owned();
        store
            .upsert_catalogue(Catalogue {
                org_id,
                id: catalogue_id,
                name: "default".into(),
                host,
                title: BTreeMap::from([
                    ("sk".into(), "Katalóg otvorených dát".into()),
                    ("cs".into(), "Katalog otevřených dat".into()),
                    ("en".into(), "Open Data Catalogue".into()),
                ]),
                description: BTreeMap::from([
                    ("sk".into(), "Katalóg dát".into()),
                    ("cs".into(), "Katalog dat".into()),
                    ("en".into(), "Data catalogue".into()),
                ]),
                homepage: None,
                default_language: "sk".into(),
                languages: vec!["sk".into(), "cs".into(), "en".into()],
                main_profile: "dcat-ap-3.0.1".into(),
                active_profiles: vec![
                    ProfilePin {
                        profile_id: "dcat-ap-3.0.1".into(),
                        version: "3.0.1".into(),
                    },
                    ProfilePin {
                        profile_id: "dcat-ap-cz".into(),
                        version: "2026-09-23".into(),
                    },
                    ProfilePin {
                        profile_id: "dcat-ap-sk-2.0".into(),
                        version: "2.0.1".into(),
                    },
                ],
                default_licence: Some("cc-by".into()),
                publisher_uri: Some("https://data.gov.sk/id/legal-subject/00000000".into()),
                publisher_name: None,
                contact_point: None,
                theme: ThemeTokens::default(),
                branding: None,
                hidden_datasets: BTreeMap::new(),
                allow_cross_org_harvest: true,
                allowed_outbound_hosts: Vec::new(),
                applicable_legislation: Vec::new(),
                modified: Utc::now(),
            })
            .await?;
    }

    Ok(())
}

/// Imports a CKAN JSON dump (an array of packages, or a `package_search` envelope `{"result": {"results": [...]}}`)
/// into `store` under `ctx`, creating missing publishers and validating each dataset.
pub async fn import_ckan_json(
    store: &dyn Store,
    ctx: &RequestCtx,
    raw_json: &str,
    default_lang: &str,
) -> Result<(usize, usize)> {
    let parsed: Value = serde_json::from_str(raw_json).map_err(|e| DataroomError::Validation {
        message: format!("invalid CKAN JSON: {e}"),
        fields: BTreeMap::new(),
    })?;

    let mut created_publishers = 0usize;
    let mut imported_datasets = 0usize;

    // 1. Import explicit `organizations` / `organization_list` entries if present in the dump (E4)
    if let Some(orgs) = parsed
        .get("organizations")
        .or_else(|| parsed.get("organization_list"))
        .and_then(Value::as_array)
    {
        for org_entry in orgs {
            let (slug, title, desc, img, initials, colour) = match org_entry {
                Value::String(s) => (s.as_str(), s.as_str(), "", None, None, None),
                Value::Object(o) => {
                    let s = o.get("name").and_then(Value::as_str).unwrap_or("");
                    let t = o.get("title").and_then(Value::as_str).unwrap_or(s);
                    let d = o.get("description").and_then(Value::as_str).unwrap_or("");
                    let im = o
                        .get("image_url")
                        .and_then(Value::as_str)
                        .filter(|u| !u.is_empty());
                    let init = o.get("initials").and_then(Value::as_str).map(str::to_owned);
                    let col = o.get("colour").and_then(Value::as_str).map(str::to_owned);
                    (s, t, d, im, init, col)
                }
                _ => continue,
            };
            if slug.is_empty() {
                continue;
            }
            if store.get_publisher(ctx, slug).await?.is_none() {
                let now = Utc::now();
                store
                    .upsert_publisher(
                        ctx,
                        Publisher {
                            org_id: ctx.org_id,
                            catalogue_id: ctx.catalogue_id,
                            id: format!("pub-{slug}"),
                            name: slug.to_owned(),
                            title: BTreeMap::from([(default_lang.to_owned(), title.to_owned())]),
                            description: if desc.is_empty() {
                                BTreeMap::new()
                            } else {
                                BTreeMap::from([(default_lang.to_owned(), desc.to_owned())])
                            },
                            uri: format!("https://data.gov.sk/id/legal-subject/{slug}"),
                            publisher_type: None,
                            identifier: None,
                            email: None,
                            homepage: None,
                            logo_url: img.map(str::to_owned),
                            contact_point: None,
                            state: RecordState::Active,
                            merged_into: None,
                            editor_grants: Vec::new(),
                            badge_initials: initials,
                            badge_colour: colour,
                            declared_by_instance: None,
                            created: now,
                            modified: now,
                        },
                    )
                    .await?;
                created_publishers += 1;
            }
        }
    }

    // 2. Import `hidden` list (`{name: reason}` or `["name1", ...]`) onto the catalogue if present (E4)
    if let Some(hidden_val) = parsed.get("hidden") {
        if let Some(mut cat) = store.get_catalogue(ctx.org_id, ctx.catalogue_id).await? {
            match hidden_val {
                Value::Object(map) => {
                    for (k, v) in map {
                        let reason = v.as_str().unwrap_or("hidden").to_owned();
                        cat.hidden_datasets.insert(k.clone(), reason);
                    }
                }
                Value::Array(arr) => {
                    for item in arr.iter().filter_map(Value::as_str) {
                        cat.hidden_datasets.insert(item.to_owned(), "hidden".into());
                    }
                }
                _ => {}
            }
            store.upsert_catalogue(cat).await?;
        }
    }

    let packages: Vec<Value> = if let Some(arr) = parsed.as_array() {
        arr.clone()
    } else if let Some(arr) = parsed.pointer("/result/results").and_then(Value::as_array) {
        arr.clone()
    } else if let Some(arr) = parsed
        .pointer("/package_search/result/results")
        .and_then(Value::as_array)
    {
        arr.clone()
    } else if let Some(arr) = parsed.get("results").and_then(Value::as_array) {
        arr.clone()
    } else if let Some(arr) = parsed.get("packages").and_then(Value::as_array) {
        arr.clone()
    } else if parsed.is_object() && parsed.get("name").is_some() {
        vec![parsed]
    } else {
        return Err(DataroomError::Validation {
            message: "expected CKAN package array or package_search envelope".into(),
            fields: BTreeMap::new(),
        });
    };

    for pkg in &packages {
        let org_slug = pkg
            .pointer("/organization/name")
            .or_else(|| pkg.get("owner_org"))
            .and_then(Value::as_str)
            .unwrap_or("default-publisher");
        let org_title = pkg
            .pointer("/organization/title")
            .and_then(Value::as_str)
            .unwrap_or(org_slug);

        if store.get_publisher(ctx, org_slug).await?.is_none() {
            let now = Utc::now();
            store
                .upsert_publisher(
                    ctx,
                    Publisher {
                        org_id: ctx.org_id,
                        catalogue_id: ctx.catalogue_id,
                        id: format!("pub-{org_slug}"),
                        name: org_slug.to_owned(),
                        title: BTreeMap::from([(default_lang.to_owned(), org_title.to_owned())]),
                        description: BTreeMap::new(),
                        uri: format!("https://data.gov.sk/id/legal-subject/{org_slug}"),
                        publisher_type: None,
                        identifier: None,
                        email: None,
                        homepage: None,
                        logo_url: None,
                        contact_point: None,
                        state: RecordState::Active,
                        merged_into: None,
                        editor_grants: Vec::new(),
                        badge_initials: None,
                        badge_colour: None,
                        declared_by_instance: None,
                        created: now,
                        modified: now,
                    },
                )
                .await?;
            created_publishers += 1;
        }

        let mut ds = dataset_from_ckan(ctx.org_id, ctx.catalogue_id, pkg, None, default_lang);
        if ds.publisher_id.is_empty() {
            ds.publisher_id = org_slug.to_owned();
        }
        store.upsert_dataset(ctx, ds).await?;
        imported_datasets += 1;
    }

    Ok((created_publishers, imported_datasets))
}

/// Runs the CLI command and returns a JSON summary.
pub async fn run_cli(cli: Cli) -> Result<Value> {
    let org_id = OrgId::parse_verified(&cli.org_id)?;
    let catalogue_id = CatalogueId::parse_verified(&cli.catalogue_id)?;

    let store: Arc<dyn Store> = match &cli.database_url {
        Some(url) => {
            let pool = sqlx::PgPool::connect(url)
                .await
                .map_err(|e| DataroomError::Storage(format!("connect postgres: {e}")))?;
            if matches!(cli.command, Command::Migrate) {
                let pg = PgStore::from_pool_unchecked(pool);
                pg.migrate().await?;
                return Ok(serde_json::json!({ "migrated": true }));
            }
            Arc::new(PgStore::connect_and_verify(pool).await?)
        }
        None => Arc::new(MemoryStore::new()),
    };

    let host = match &cli.command {
        Command::Serve { host, .. } => host.clone(),
        _ => "opendata.localhost".into(),
    };
    ensure_default_catalogue(store.as_ref(), org_id, catalogue_id, &host).await?;

    let ctx = RequestCtx::new(
        org_id,
        catalogue_id,
        Subject::authenticated("cli-operator", vec![org_id], Vec::new()),
    );

    match cli.command {
        Command::Migrate => Ok(serde_json::json!({ "migrated": true, "store": "memory" })),
        Command::Serve { listen, .. } => {
            let authz = Arc::new(PolicyAuthorizer::default());
            let state = DataroomState::new(store, authz, org_id, catalogue_id);
            let app = router(state);
            let listener = tokio::net::TcpListener::bind(&listen)
                .await
                .map_err(|e| DataroomError::Storage(format!("bind {listen}: {e}")))?;
            axum::serve(listener, app)
                .await
                .map_err(|e| DataroomError::Storage(format!("serve: {e}")))?;
            Ok(serde_json::json!({ "served": listen }))
        }
        Command::Harvest { source } => {
            let src = store
                .get_harvest_source(&ctx, &source)
                .await?
                .ok_or_else(|| {
                    DataroomError::NotFound(format!("harvest source '{source}' not found"))
                })?;
            let engine = HarvestEngine::new(Arc::new(HttpFetcher::new()));
            let run = engine.run_source(&ctx, store.as_ref(), &src).await?;
            Ok(serde_json::to_value(run).unwrap_or(Value::Null))
        }
        Command::FeedCheck { profile } => {
            let cat = store
                .get_catalogue(org_id, catalogue_id)
                .await?
                .ok_or_else(|| DataroomError::NotFound("catalogue not found".into()))?;
            let datasets = store.list_datasets(&ctx).await?;
            let publishers = store.list_publishers(&ctx).await?;
            let pub_map: BTreeMap<String, Publisher> = publishers
                .into_iter()
                .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p)])
                .collect();
            let registry = ProfileRegistry::new();
            registry.check_catalogue_feed(&cat, &datasets, &pub_map, profile.as_deref())
        }
        Command::ImportCkan { file } => {
            let raw = std::fs::read_to_string(&file).map_err(|e| {
                DataroomError::Storage(format!("failed to read {}: {e}", file.display()))
            })?;
            let (pubs, dss) = import_ckan_json(store.as_ref(), &ctx, &raw, "sk").await?;
            Ok(serde_json::json!({
                "publishersCreated": pubs,
                "datasetsImported": dss,
            }))
        }
        Command::Op { name, input } => {
            let input_val: Value = serde_json::from_str(&input)
                .map_err(|e| DataroomError::field_error("input", format!("invalid JSON: {e}")))?;
            let authz = Arc::new(PolicyAuthorizer::new(vec![dataroom_core::GrantRule {
                org_id,
                catalogue_id: Some(catalogue_id),
                principal: "cli-operator".into(),
                actions: vec![dataroom_core::Action::Admin],
                publisher_id: None,
            }]));
            let state = DataroomState::new(store, authz, org_id, catalogue_id);
            execute_operation(&state.op_context(), &ctx, &name, input_val).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn import_ckan_and_feed_check_work_end_to_end() {
        let org_id = OrgId::from_uuid(Uuid::from_u128(1));
        let cat_id = CatalogueId::from_uuid(Uuid::from_u128(10));
        let store = MemoryStore::new();
        let ctx = RequestCtx::new(
            org_id,
            cat_id,
            Subject::authenticated("admin", vec![org_id], Vec::new()),
        );
        let ckan_dump = serde_json::json!({
            "success": true,
            "result": {
                "count": 1,
                "results": [
                    {
                        "name": "bb-ovzdusie",
                        "title": "Kvalita ovzdušia",
                        "notes": "Merania kvality ovzdušia",
                        "license_id": "cc-by",
                        "organization": { "name": "banskabystrica", "title": "Mesto Banská Bystrica" },
                        "tags": [{ "name": "ovzdusie" }],
                        "resources": [
                            { "name": "CSV", "url": "https://data.banskabystrica.sk/air.csv", "format": "CSV" }
                        ]
                    }
                ]
            }
        });
        let (pubs, dss) = import_ckan_json(&store, &ctx, &ckan_dump.to_string(), "sk")
            .await
            .expect("import ckan");
        assert_eq!(pubs, 1);
        assert_eq!(dss, 1);
        let imported = store
            .get_dataset(&ctx, "bb-ovzdusie")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(imported.publisher_id, "banskabystrica");
    }

    #[tokio::test]
    async fn e4_import_ckan_preserves_names_publishers_resources_extras_and_hidden_list() {
        let org_id = OrgId::from_uuid(Uuid::from_u128(1));
        let cat_id = CatalogueId::from_uuid(Uuid::from_u128(10));
        let store = MemoryStore::new();
        store
            .upsert_catalogue(Catalogue {
                org_id,
                id: cat_id,
                name: "dev".into(),
                host: "opendata.dev.joinedcontext.com".into(),
                title: BTreeMap::from([("sk".into(), "Otvorené dáta".into())]),
                description: BTreeMap::new(),
                homepage: None,
                default_language: "sk".into(),
                languages: vec!["sk".into(), "en".into()],
                main_profile: "dcat-ap-3.0.1".into(),
                active_profiles: Vec::new(),
                default_licence: Some("cc-by".into()),
                publisher_uri: None,
                publisher_name: None,
                contact_point: None,
                theme: ThemeTokens::default(),
                branding: None,
                hidden_datasets: BTreeMap::new(),
                allow_cross_org_harvest: true,
                allowed_outbound_hosts: Vec::new(),
                applicable_legislation: Vec::new(),
                modified: Utc::now(),
            })
            .await
            .unwrap();

        let ctx = RequestCtx::new(
            org_id,
            cat_id,
            Subject::authenticated("admin", vec![org_id], Vec::new()),
        );
        let dump = serde_json::json!({
            "organization_list": [
                {
                    "name": "banskabystrica",
                    "title": "Mesto Banská Bystrica",
                    "description": "Otvorené údaje mesta Banská Bystrica.",
                    "initials": "BB",
                    "colour": "#00579c"
                },
                {
                    "name": "barcelona",
                    "title": "Ajuntament de Barcelona",
                    "description": "Otvorené údaje mesta Barcelona (Open Data BCN, CC BY 4.0), publikované cez endpointy joinedcontext.",
                    "initials": "BCN",
                    "colour": "#c8102e"
                }
            ],
            "hidden": {
                "draft-internal-pkg": "incomplete quality check"
            },
            "result": {
                "count": 2,
                "results": [
                    {
                        "id": "d262a2d3-17b2-4e44-afb7-1275fdbf3e4e",
                        "name": "air-quality",
                        "title": "Qualitat de l'aire a Barcelona",
                        "notes": "Les estacions de la xarxa de qualitat de l'aire de Barcelona",
                        "owner_org": "barcelona",
                        "license_id": "cc-by",
                        "tags": [{ "name": "NO2" }, { "name": "PM10" }],
                        "resources": [
                            {
                                "id": "0e74d5fe-c17f-4bd7-879b-0c3bf6481817",
                                "name": "CSV",
                                "url": "https://api.dev.joinedcontext.com/api/endpoint/esbxdrr7snorjsp7uus2luihr6vl5vv6/file.csv",
                                "format": "CSV"
                            }
                        ],
                        "extras": [
                            { "key": "endpoint", "value": "https://api.dev.joinedcontext.com/api/endpoint/esbxdrr7snorjsp7uus2luihr6vl5vv6/" },
                            { "key": "spatial_uri", "value": "http://data.europa.eu/nuts/code/ES511" }
                        ]
                    },
                    {
                        "name": "draft-internal-pkg",
                        "title": "Hidden package",
                        "notes": "Should be in hidden_datasets",
                        "owner_org": "banskabystrica"
                    }
                ]
            }
        });

        let (pubs, dss) = import_ckan_json(&store, &ctx, &dump.to_string(), "sk")
            .await
            .expect("import full dump");
        assert_eq!(pubs, 2);
        assert_eq!(dss, 2);

        let cat = store.get_catalogue(org_id, cat_id).await.unwrap().unwrap();
        assert_eq!(
            cat.hidden_datasets
                .get("draft-internal-pkg")
                .map(String::as_str),
            Some("incomplete quality check")
        );

        let ds = store
            .get_dataset(&ctx, "air-quality")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ds.id, "d262a2d3-17b2-4e44-afb7-1275fdbf3e4e");
        assert_eq!(ds.publisher_id, "barcelona");
        assert_eq!(ds.distributions.len(), 1);
        assert_eq!(
            ds.distributions[0].id,
            "0e74d5fe-c17f-4bd7-879b-0c3bf6481817"
        );
        assert_eq!(
            ds.endpoint_url.as_deref(),
            Some(
                "https://api.dev.joinedcontext.com/api/endpoint/esbxdrr7snorjsp7uus2luihr6vl5vv6/"
            )
        );
    }
}
