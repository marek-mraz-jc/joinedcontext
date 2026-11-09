//! dataroom (dataset catalogue) inside `jc`: the default catalogue on Postgres when
//! `[dataroom.database]` is set (`dataroom_store_pg::PgStore::connect_and_verify`, the boot check
//! refusing a role with SUPERUSER or BYPASSRLS and any `dataroom` table without forced RLS; with
//! a migration role, `jc` first migrates under dataroom's advisory lock and grants the request
//! role its table rights), else in memory; the policy authorizer of the standalone `dataroom
//! serve`, URLs under the mount prefix. When the
//! Portal runs beside it, first-start setup holds a CKAN API token for the Portal: the subject
//! `portal-ckan`, Read and Propose on the default catalogue only
//! (`dataroom_api::portal_publisher`).

use crate::app::StartError;
use crate::config::{DataroomSection, Given, HarvestSeed, OrganizationSection};
use crate::module::Module;
use crate::setup::{Secret, Setup};
use crate::siblings::Siblings;
use axum::Router;
use dataroom_api::DataroomState;
use dataroom_core::branding::{BrandingAddresses, BrandingBlock};
use dataroom_core::{
    CatalogueId, DataroomError, HarvestSource, HarvestSourceKind, OrgId, PolicyAuthorizer,
    Publisher, RecordState, RequestCtx, Store, Subject,
};
use dataroom_harvest::{HarvestEngine, HarvestScheduler, HttpFetcher};
use dataroom_store_memory::MemoryStore;
use dataroom_store_pg::PgStore;
use jc_router::Mount;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The configured store: Postgres (refused unless the boot check passes) or memory.
async fn open_store(cfg: &DataroomSection) -> Result<Arc<dyn Store>, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    let Some(db) = &cfg.database else {
        return Ok(Arc::new(MemoryStore::new()));
    };
    if let Some(opts) = db.migrate_options().map_err(|e| err(&e))? {
        let pool = crate::pg::connect(opts, 1).await.map_err(|e| err(&e))?;
        PgStore::from_pool_unchecked(pool.clone())
            .migrate()
            .await
            .map_err(|e| err(&e))?;
        // dataroom's migration names no request role: the owner grants the configured one.
        let role = db.app_role().map_err(|e| err(&e))?;
        let quoted = format!("\"{}\"", role.replace('"', "\"\""));
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "GRANT USAGE ON SCHEMA dataroom TO {quoted};
             GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA dataroom TO {quoted};"
        )))
        .execute(&pool)
        .await
        .map_err(|e| err(&format!("grant to {role}: {e}")))?;
        pool.close().await;
    }
    let pool = crate::pg::connect(db.app_options().map_err(|e| err(&e))?, db.max_connections())
        .await
        .map_err(|e| err(&e))?;
    Ok(Arc::new(
        PgStore::connect_and_verify(pool)
            .await
            .map_err(|e| err(&e))?,
    ))
}

/// The assistant widget the catalogue docks (G2): the assistant's page of `deployment` when the
/// module runs in this `jc`; `None` otherwise, so only an assistant address named in
/// `[dataroom.addresses]` (or none) points a launcher anywhere.
#[must_use]
pub fn assistant_widget(siblings: &Siblings, deployment: &str) -> Option<String> {
    siblings
        .get(Module::Assistant)
        .map(|a| format!("{}/d/{deployment}/widget", a.public))
}

/// The installation branding `jc` hands dataroom: only the live parts' addresses (G1), named in
/// `[dataroom.addresses]` or taken from the mounted modules, and the assistant widget of jc's own
/// assistant module. A catalogue's own branding still wins per catalogue; one that nobody serves
/// stays empty, so no page points at it.
fn installation_branding(
    live: &crate::siblings::LiveAddresses,
    assistant_widget: Option<String>,
) -> BrandingBlock {
    BrandingBlock {
        addresses: BrandingAddresses {
            viz: live.viz.clone(),
            api: live.api.clone(),
            tiles: live.tiles.clone(),
            assistant: live.assistant.clone(),
            portal: live.portal.clone(),
        },
        assistant_widget,
        ..BrandingBlock::default()
    }
}

pub async fn build(
    cfg: &DataroomSection,
    organizations: &[OrganizationSection],
    tokens: &BTreeMap<String, Given>,
    setup: &Setup,
    siblings: &Siblings,
    mount: Mount,
    assistant_widget: Option<String>,
) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    let org_id = OrgId::parse_verified(&cfg.org_id).map_err(|e| err(&e))?;
    let catalogue_id = CatalogueId::parse_verified(&cfg.catalogue_id).map_err(|e| err(&e))?;
    let store = open_store(cfg).await?;
    dataroom_server::ensure_default_catalogue(store.as_ref(), org_id, catalogue_id, &mount.host)
        .await
        .map_err(|e| err(&e))?;
    let (mut grants, mut bindings) = match setup.value(Secret::CatalogueToken) {
        None => (Vec::new(), Vec::new()),
        Some(token) => {
            let (binding, grant) =
                dataroom_api::portal_publisher(token, org_id, catalogue_id).map_err(|e| err(&e))?;
            (vec![grant], vec![binding])
        }
    };
    let live = crate::siblings::live_addresses(&cfg.addresses, siblings);
    let mut by_catalogue = BTreeMap::new();
    for org in organizations {
        let id = CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| err(&e))?;
        if id == catalogue_id {
            return Err(err(&format!(
                "[[organizations]] {}: catalogue {id} is the [dataroom] default catalogue",
                org.name
            )));
        }
        let host = bare_host(&org.hosts.opendata);
        dataroom_server::ensure_catalogue(store.as_ref(), org_id, id, &org.name, host)
            .await
            .map_err(|e| err(&e))?;
        ensure_publisher(store.as_ref(), org_id, id, org, &mount.scheme)
            .await
            .map_err(|e| err(&e))?;
        if let Some(token) = tokens.get(&org.name) {
            let token = token.secret.expose();
            if bindings
                .iter()
                .any(|b: &dataroom_api::TokenBinding| b.token == token.trim())
            {
                return Err(err(&format!(
                    "[[organizations]] {}: its Portal token is another catalogue's",
                    org.name
                )));
            }
            let (binding, grant) = dataroom_api::portal_publisher_on(token, org_id, id, &org.name)
                .map_err(|e| err(&e))?;
            bindings.push(binding);
            grants.push(grant);
        }
        by_catalogue.insert(
            id,
            organization_branding(&live, org, &mount.scheme, assistant_widget.clone()),
        );
    }
    let mut catalogues = vec![(org_id, catalogue_id)];
    for org in organizations {
        catalogues.push((
            org_id,
            CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| err(&e))?,
        ));
    }
    for &(o, c) in &catalogues {
        grants.push(dataroom_harvest::harvest_principal(o, c).1);
    }
    for seed in &cfg.harvest {
        let (cat, site) = match &seed.organization {
            None => (catalogue_id, mount.public_base()),
            Some(name) => {
                let org = organizations
                    .iter()
                    .find(|o| &o.name == name)
                    .ok_or_else(|| {
                        err(&format!(
                            "[[dataroom.harvest]] {}: no [[organizations]] {name}",
                            seed.name
                        ))
                    })?;
                (
                    CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| err(&e))?,
                    format!("{}://{}", mount.scheme, org.hosts.opendata),
                )
            }
        };
        seed_harvest(store.as_ref(), org_id, cat, seed, &site)
            .await
            .map_err(|e| err(&format!("[[dataroom.harvest]] {}: {e}", seed.name)))?;
    }
    let authorizer = Arc::new(PolicyAuthorizer::new(grants));
    HarvestScheduler::new(
        Arc::new(HarvestEngine::new(Arc::new(HttpFetcher::new()))),
        store.clone(),
        authorizer.clone(),
        catalogues,
    )
    .spawn();
    let state = DataroomState::new(store, authorizer, org_id, catalogue_id)
        .with_tokens(bindings)
        .with_installation_branding(installation_branding(&live, assistant_widget))
        .with_catalogue_branding(by_catalogue)
        // Several catalogues by host: a loopback name names none of them.
        .with_loopback_default(organizations.is_empty())
        .with_public_base(mount.public_base())
        .with_mount_prefix(mount.path_prefix);
    Ok(dataroom_api::router(state))
}

fn bare_host(host: &str) -> &str {
    host.split(':').next().unwrap_or(host)
}

/// An Organization's catalogue frames its own Portal and API: the installation's other live
/// parts stay as they are.
fn organization_branding(
    live: &crate::siblings::LiveAddresses,
    org: &OrganizationSection,
    scheme: &str,
    assistant_widget: Option<String>,
) -> BrandingBlock {
    let mut block = installation_branding(live, assistant_widget);
    block.addresses.api = format!("{scheme}://{}", org.hosts.api);
    block.addresses.portal = format!("{scheme}://{}", org.hosts.portal);
    block
}

/// The CKAN organisation of the JC Organization in its catalogue (ADR-N-058 2.6), held by its
/// name: the one publisher the Portal's grant there may publish under.
async fn ensure_publisher(
    store: &dyn Store,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    org: &OrganizationSection,
    scheme: &str,
) -> dataroom_core::Result<()> {
    let ctx = RequestCtx::new(org_id, catalogue_id, Subject::anonymous());
    if store.get_publisher(&ctx, &org.name).await?.is_some() {
        return Ok(());
    }
    let now = chrono::Utc::now();
    store
        .upsert_publisher(
            &ctx,
            Publisher {
                org_id,
                catalogue_id,
                id: String::new(),
                name: org.name.clone(),
                title: BTreeMap::from([("en".to_owned(), org.name.clone())]),
                description: BTreeMap::new(),
                uri: format!("{scheme}://{}/publisher/{}", org.hosts.opendata, org.name),
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
    Ok(())
}

/// Holds one `[[dataroom.harvest]]` source in catalogue `catalogue_id`, whose public base is
/// `site`: its publisher created when missing, the source written over the stored one. A
/// source naming a scheme other than https, a URL or gateway host outside its own
/// `allowed_hosts`, a schedule that is not cron or a converted file that does not read is
/// refused.
pub async fn seed_harvest(
    store: &dyn Store,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    seed: &HarvestSeed,
    site: &str,
) -> dataroom_core::Result<HarvestSource> {
    let kind: HarvestSourceKind =
        serde_json::from_value(serde_json::Value::from(seed.kind.as_str())).map_err(|_| {
            DataroomError::field_error("kind", format!("unknown kind '{}'", seed.kind))
        })?;
    dataroom_harvest::validate_outbound_url(&seed.url, &seed.allowed_hosts, &[], false)?;
    dataroom_harvest::cron_matches(&seed.schedule, chrono::Utc::now())?;
    if let Some(gateway) = &seed.gateway {
        dataroom_harvest::validate_outbound_url(gateway, &seed.allowed_hosts, &[], false)?;
    }
    let converted = match &seed.converted_file {
        None => BTreeMap::new(),
        Some(path) => converted_datasets(path, seed.datasets_base.as_deref(), site)?,
    };
    let ctx = RequestCtx::new(org_id, catalogue_id, Subject::anonymous());
    let now = chrono::Utc::now();
    if store.get_publisher(&ctx, &seed.publisher).await?.is_none() {
        store
            .upsert_publisher(
                &ctx,
                Publisher {
                    org_id,
                    catalogue_id,
                    id: String::new(),
                    name: seed.publisher.clone(),
                    title: BTreeMap::from([("sk".to_owned(), seed.publisher_title.clone())]),
                    description: seed
                        .publisher_description
                        .iter()
                        .map(|d| ("sk".to_owned(), d.clone()))
                        .collect(),
                    uri: seed.publisher_uri.clone(),
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
    }
    store
        .upsert_harvest_source(
            &ctx,
            HarvestSource {
                org_id,
                catalogue_id,
                id: format!("hs-{}", seed.name),
                name: seed.name.clone(),
                kind,
                url: seed.url.clone(),
                publisher_id: seed.publisher.clone(),
                target_publisher_iri: Some(seed.publisher_uri.clone()),
                spatial_uri: seed.spatial_uri.clone(),
                name_prefix: seed.name_prefix.clone(),
                allowed_hosts: seed.allowed_hosts.clone(),
                rewrite_hosts: seed.rewrite_hosts.clone(),
                skip_items: seed.skip.clone(),
                title_overrides: seed.titles.clone(),
                spelling_fixes: seed.spelling.clone(),
                require_column: seed.require_column.clone(),
                schedule: seed.schedule.clone(),
                enabled: seed.enabled,
                converted,
                gateway: seed.gateway.clone(),
                modified: now,
            },
        )
        .await
}

/// The converted datasets of `path` (prod's `od-datasets.json`), each `dataset` URL moved from
/// `base` to this catalogue's `site`, as prod's harvest CronJob replaces `datasetsBase`.
fn converted_datasets(
    path: &std::path::Path,
    base: Option<&str>,
    site: &str,
) -> dataroom_core::Result<BTreeMap<String, dataroom_core::ConvertedDataset>> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        DataroomError::field_error(
            "converted_file",
            format!("{}: {}", path.display(), e.kind()),
        )
    })?;
    let mut all: BTreeMap<String, dataroom_core::ConvertedDataset> = serde_json::from_str(&raw)
        .map_err(|e| {
            DataroomError::field_error("converted_file", format!("{}: {e}", path.display()))
        })?;
    if let Some(base) = base.filter(|b| !b.is_empty()) {
        let to = format!("{}/", site.trim_end_matches('/'));
        for entry in all.values_mut() {
            entry.dataset = entry.dataset.replace(base, &to);
        }
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dataroom_core::HarvestRunState;
    use dataroom_core::ops::HarvestRunner;
    use dataroom_harvest::{MapFetcher, validate_outbound_url};
    use std::collections::BTreeSet;

    /// prod's open data catalogue: the base its datasets' `isPartOf` names.
    const SITE: &str = "https://opendata.smartcity.joinedcontext.com";
    const GATEWAY: &str = "https://api.smartcity.joinedcontext.com";

    /// The `[[dataroom.harvest]]` block of prod's two sources, as jc.toml carries it.
    fn prod_seeds() -> Vec<HarvestSeed> {
        #[derive(serde::Deserialize)]
        struct File {
            dataroom: DataroomSection,
        }
        let raw = include_str!("../../docs/harvest-sources.toml");
        toml::from_str::<File>(raw)
            .expect("the block parses")
            .dataroom
            .harvest
    }

    async fn catalogue() -> (MemoryStore, RequestCtx) {
        let d = DataroomSection::default();
        let org = OrgId::parse_verified(&d.org_id).unwrap();
        let cat = CatalogueId::parse_verified(&d.catalogue_id).unwrap();
        let store = MemoryStore::new();
        dataroom_server::ensure_default_catalogue(&store, org, cat, "opendata.example")
            .await
            .expect("catalogue");
        let (subject, _) = dataroom_harvest::harvest_principal(org, cat);
        (store, RequestCtx::new(org, cat, subject))
    }

    #[tokio::test]
    async fn prods_two_sources_seed_with_their_publishers_and_only_their_hosts() {
        let seeds = prod_seeds();
        assert_eq!(
            seeds
                .iter()
                .map(|s| (s.name.as_str(), s.kind.as_str(), s.schedule.as_str()))
                .collect::<Vec<_>>(),
            [
                ("nkod-banskabystrica", "nkod-sparql", "23 3 * * *"),
                ("hub-bbsk", "arcgis-hub", "23 3 * * *")
            ]
        );
        let (store, ctx) = catalogue().await;
        for seed in &seeds {
            let src = seed_harvest(&store, ctx.org_id, ctx.catalogue_id, seed, SITE)
                .await
                .expect("seeded");
            assert!(src.enabled);
            let publisher = store
                .get_publisher(&ctx, &seed.publisher)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(publisher.uri, seed.publisher_uri);
            // The source's own host is admitted; any other host, an internal one included, is not.
            validate_outbound_url(&src.url, &src.allowed_hosts, &[], false).expect("own host");
            for refused in [
                "https://data.gov.sk/api/sparql",
                "https://evil.example/x",
                "https://127.0.0.1/x",
                "https://opendata.smartcity.joinedcontext.com/api/3/action/package_list",
                "http://data.slovensko.sk/api/sparql",
            ] {
                assert!(
                    validate_outbound_url(refused, &src.allowed_hosts, &[], false).is_err(),
                    "{}: {refused}",
                    seed.name
                );
            }
        }
        assert_eq!(store.list_harvest_sources(&ctx).await.unwrap().len(), 2);
        // Seeding again on the next start writes over the stored source, nothing doubles.
        seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &seeds[0], SITE)
            .await
            .unwrap();
        assert_eq!(store.list_harvest_sources(&ctx).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_seed_outside_its_hosts_or_off_cron_is_refused() {
        let (store, ctx) = catalogue().await;
        let mut off_list = prod_seeds()[0].clone();
        off_list.allowed_hosts.retain(|h| h != "data.slovensko.sk");
        assert!(
            seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &off_list, SITE)
                .await
                .is_err()
        );
        let mut bad_cron = prod_seeds()[1].clone();
        bad_cron.schedule = "nightly".into();
        assert!(
            seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &bad_cron, SITE)
                .await
                .is_err()
        );
        let mut bad_kind = prod_seeds()[1].clone();
        bad_kind.kind = "ftp".into();
        assert!(
            seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &bad_kind, SITE)
                .await
                .is_err()
        );
        assert!(store.list_harvest_sources(&ctx).await.unwrap().is_empty());
    }

    /// Both sources run once against what their hosts answered on 2026-11-09 (recorded fixtures,
    /// `tests/fixtures/harvest/`: NKOD's SPARQL answer, the Hub's two search pages, prod's
    /// `od-datasets.json` and its gateway's `schema/index.json` of each Endpoint): every fetch
    /// goes to an allow-listed host, and the datasets prod's CKAN serves for the two publishers
    /// are among those listed, each with prod's distributions.
    #[tokio::test]
    async fn prods_sources_harvest_from_recorded_answers() {
        let (store, ctx) = catalogue().await;
        let fetcher = MapFetcher::new();
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/harvest");
        let mut seeds = prod_seeds();
        for seed in &mut seeds {
            seed.converted_file = Some(fixtures.join("od-datasets.json"));
            seed.datasets_base = Some("https://opendata.dev.joinedcontext.com/".into());
            seed.gateway = Some(GATEWAY.into());
            seed.allowed_hosts
                .push("api.smartcity.joinedcontext.com".into());
        }
        let nkod = seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &seeds[0], SITE)
            .await
            .unwrap();
        let hub = seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &seeds[1], SITE)
            .await
            .unwrap();
        fetcher.insert(
            dataroom_harvest::runner::nkod_query_url(&nkod).unwrap(),
            Some("application/sparql-results+json"),
            include_str!("../../tests/fixtures/harvest/nkod-banskabystrica.json"),
        );
        let pages: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../tests/fixtures/harvest/hub-bbsk.json"))
                .unwrap();
        for page in pages {
            fetcher.insert(
                page["url"].as_str().unwrap(),
                Some("application/json"),
                page["body"].to_string(),
            );
        }
        let indexes: BTreeMap<String, serde_json::Value> = serde_json::from_str(include_str!(
            "../../tests/fixtures/harvest/model-index.json"
        ))
        .unwrap();
        for (endpoint, index) in indexes {
            fetcher.insert(
                format!("{GATEWAY}/api/endpoint/{endpoint}/schema/index.json"),
                Some("application/json"),
                index.to_string(),
            );
        }
        let engine = HarvestEngine::new(Arc::new(fetcher.clone())).with_custom_dns(|_, port| {
            Ok(vec![std::net::SocketAddr::from(([93, 184, 216, 34], port))])
        });
        for src in [&nkod, &hub] {
            let run = engine.run_source(&ctx, &store, src).await.expect("run");
            assert_eq!(run.state, HarvestRunState::Succeeded);
            eprintln!(
                "{}: created {} skipped {}",
                src.name,
                run.created_count,
                run.skipped.len()
            );
        }
        let hosts: BTreeSet<String> = fetcher.pinned_seen().into_iter().map(|(h, _)| h).collect();
        assert_eq!(
            hosts,
            BTreeSet::from([
                "api.smartcity.joinedcontext.com".to_owned(),
                "data.slovensko.sk".to_owned(),
                "opendata.bbsk.sk".to_owned()
            ])
        );
        let datasets = store.list_datasets(&ctx).await.unwrap();
        let listed: BTreeMap<&str, &dataroom_core::Dataset> =
            datasets.iter().map(|d| (d.name.as_str(), d)).collect();
        // What prod's CKAN answered for every converted name of the two publishers on 2026-11-09:
        // its organisation (served), 403 (the publish gate hides it) or 404.
        let prod: BTreeMap<String, serde_json::Value> = serde_json::from_str(include_str!(
            "../../tests/fixtures/harvest/prod-presence.json"
        ))
        .unwrap();
        let served: Vec<(&String, &str)> = prod
            .iter()
            .filter_map(|(name, org)| org.as_str().map(|o| (name, o)))
            .collect();
        assert_eq!(served.len(), 17);
        for (name, org) in &served {
            let ds = listed
                .get(name.as_str())
                .unwrap_or_else(|| panic!("{name} is not listed"));
            assert_eq!(&ds.publisher_id, org, "{name}");
            assert!(
                ds.endpoint_url
                    .as_deref()
                    .is_some_and(|u| u.starts_with(GATEWAY)),
                "{name}"
            );
            assert!(
                ds.distributions.len() >= 12,
                "{name}: {}",
                ds.distributions.len()
            );
            assert_eq!(
                ds.extras.get("harvest_source").map(String::as_str),
                Some(if *org == "bbsk" {
                    "hub-bbsk"
                } else {
                    "nkod-banskabystrica"
                })
            );
        }
        let more: Vec<&&str> = listed
            .keys()
            .filter(|n| !prod.get(**n).is_some_and(|o| o.is_string()))
            .collect();
        eprintln!(
            "listed {} of which prod serves {}; listed beyond prod's served: {more:?}",
            listed.len(),
            served.len()
        );
        // Nothing but converted datasets: each listed name is one prod's mapping converted.
        assert!(listed.keys().all(|n| prod.contains_key(*n)), "{more:?}");
    }
}
