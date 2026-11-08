#![forbid(unsafe_code)]
//! The Portal's CKAN calls (PORTAL-CALLS.md) replayed against the router over a real socket.
//!
//! The writes go through the Portal's own client and publisher (`jcctl::publish::ckan_http::
//! HttpCkan`, `publish`, `publish_views`, `withdraw`), exactly as `reconciler/ckan.rs` drives
//! them; the reads the Portal makes anonymously (`api/catalogue.rs` `search`/`dataset`) and the
//! stray sweep (`reconciler/ckan.rs` `sweep_with`) are sent as those functions send them, and
//! the test asserts the fields they parse.

use chrono::Utc;
use dataroom_api::{
    mount, portal_publisher, portal_publisher_from_file, DataroomState, MIN_PORTAL_TOKEN_LEN,
};
use dataroom_core::{
    Action, Catalogue, CatalogueId, ContactKind, ContactPoint, OrgId, PolicyAuthorizer, ProfilePin,
    Store, ThemeTokens,
};
use dataroom_store_memory::MemoryStore;
use jc_core::kinds::ckan::CkanInstanceSpec;
use jc_core::ngsi_catalog::Package;
use jcctl::loader::RawManifest;
use jcctl::publish::ckan::{
    publish, publish_dataset, publish_views, withdraw, CkanApi, CkanError, Outcome, PublishError,
    Settings, GENERATOR,
};
use jcctl::publish::ckan_http::HttpCkan;
use jcctl::secrets::SecretValue;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

const TOKEN: &str = "portal-ckan-token-from-the-secret-file-0123456789";
const ORGANIZATION: &str = "mesto-banska-bystrica";
const PUBLIC_DATASET: &str = "kvalita-ovzdusia";
const CLOSED_DATASET: &str = "kvalita-ovzdusia-interna";

fn catalogue(org_id: OrgId, id: CatalogueId) -> Catalogue {
    Catalogue {
        org_id,
        id,
        name: "default".into(),
        host: "opendata.city.example".into(),
        title: BTreeMap::from([
            ("sk".into(), "Katalóg".into()),
            ("en".into(), "Catalogue".into()),
        ]),
        description: BTreeMap::from([("sk".into(), "Popis".into())]),
        homepage: None,
        default_language: "sk".into(),
        languages: vec!["sk".into(), "en".into()],
        main_profile: "dcat-ap-3.0.1".into(),
        active_profiles: vec![ProfilePin {
            profile_id: "dcat-ap-3.0.1".into(),
            version: "3.0.1".into(),
        }],
        default_licence: None,
        publisher_uri: None,
        publisher_name: None,
        contact_point: Some(ContactPoint {
            uri: None,
            kind: ContactKind::Organization,
            name: BTreeMap::from([("sk".into(), "Oddelenie".into())]),
            email: "opendata@city.example".into(),
            url: None,
            identifier: None,
        }),
        theme: ThemeTokens::default(),
        branding: None,
        hidden_datasets: BTreeMap::new(),
        allow_cross_org_harvest: false,
        allowed_outbound_hosts: Vec::new(),
        applicable_legislation: Vec::new(),
        modified: Utc::now(),
        public_base: None,
    }
}

/// The router mounted at `/opendata` as `jc` mounts it, served on a loopback socket; the base
/// URL the Portal's `JC_PORTAL_CKAN_API_URL` would name.
async fn serve() -> String {
    let org_id = OrgId::from_uuid(Uuid::from_u128(0x7100));
    let cat_id = CatalogueId::from_uuid(Uuid::from_u128(0x0007_1001));
    let store = Arc::new(MemoryStore::new());
    store
        .upsert_catalogue(catalogue(org_id, cat_id))
        .await
        .expect("catalogue");
    // The binding `jc` gives the Portal (`portal_publisher_from_file`): Read and Propose only.
    let (binding, grant) = portal_publisher(TOKEN, org_id, cat_id).expect("a valid token");
    let authorizer = Arc::new(PolicyAuthorizer::new(vec![grant]));
    let state = DataroomState::new(store, authorizer, org_id, cat_id).with_tokens(vec![binding]);
    let app = mount("/opendata", state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/opendata")
}

fn endpoint(name: &str, dataset: &str, audience: &str, slug: &str) -> RawManifest {
    serde_json::from_value(json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Endpoint",
        "metadata": { "name": name, "namespace": "ovzdusie" },
        "spec": {
            "contextSpaceRef": "ovzdusie",
            "slug": slug,
            "audience": audience,
            "enabledRepresentations": ["ngsi-ld", "csv"],
            "publish": { "ckan": {
                "instanceRef": { "kind": "CkanInstance", "name": "open-data" },
                "organization": ORGANIZATION,
                "name": dataset,
            }},
            "catalog": { "visualizations": [{
                "name": "no2-bar",
                "title": { "sk": "NO2", "en": "NO2 (en)" },
                "description": { "sk": "Popis." },
                "data": { "source": "csv", "type": "AirQualityObserved" },
                "spec": {
                    "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
                    "data": { "name": "source" },
                    "mark": "bar",
                    "encoding": {
                        "x": { "field": "name", "type": "nominal" },
                        "y": { "field": "no2", "type": "quantitative" }
                    }
                }
            }]}
        }
    }))
    .expect("the manifest parses")
}

fn instance() -> CkanInstanceSpec {
    serde_json::from_value(json!({
        "url": "https://opendata.city.example",
        "organizationDefault": ORGANIZATION,
        "apiTokenRef": { "name": "ckan-open-data", "key": "apiToken" },
    }))
    .expect("the instance spec parses")
}

fn record(title: &str) -> Value {
    json!({
        "@context": "https://www.w3.org/ns/dcat.jsonld",
        "@type": "dcat:Dataset",
        "dct:identifier": "ovzdusie-public",
        "dct:title": [
            { "@value": title, "@language": "sk" },
            { "@value": format!("{title} (en)"), "@language": "en" }
        ],
        "dct:description": [{ "@value": "Hourly air quality observations.", "@language": "en" }],
        "dcat:keyword": ["ovzdusie", "air-quality"],
        "dct:license": "https://creativecommons.org/licenses/by/4.0/",
        "dct:accrualPeriodicity": "http://purl.org/cld/freq/hourly",
        "dct:publisher": "Mesto Banská Bystrica"
    })
}

fn settings(title: &str) -> Settings {
    Settings::new("data.city.example").declared_title(title)
}

/// `reconciler/ckan.rs` `one`: `HttpCkan::new(&api_url, token)`. A `SecretValue` is made only
/// by jcctl's secret backends or from the environment, so the token goes through a variable.
fn client(base: &str, token: &str) -> HttpCkan {
    let variable = format!("DATAROOM_PORTAL_CONTRACT_{}", token.len());
    std::env::set_var(&variable, token);
    let secret = SecretValue::from_env(&variable).expect("the token");
    HttpCkan::new(base, secret).expect("a client")
}

/// `api/catalogue.rs` `action_with`: an anonymous GET, `result` read only on `success: true`.
async fn anonymous(base: &str, action: &str, query: &[(&str, &str)]) -> Option<Value> {
    let answer = reqwest::Client::new()
        .get(format!("{base}/api/3/action/{action}"))
        .query(query)
        .send()
        .await
        .expect("dataroom answers");
    let body: Value = answer.json().await.expect("a CKAN document");
    (body["success"] == json!(true)).then(|| body["result"].clone())
}

/// `api/catalogue.rs` `dataset`: the public dataset `name`, as the Portal accepts it.
async fn portal_dataset(base: &str, name: &str) -> Option<Value> {
    anonymous(base, "package_show", &[("id", name)])
        .await
        .filter(|found| found["private"] != json!(true) && found["name"] == json!(name))
}

/// `api/catalogue.rs` `search`: the names of what a person searching `q` sees.
async fn portal_search(base: &str, q: &str) -> Vec<String> {
    let result = anonymous(
        base,
        "package_search",
        &[("q", q), ("rows", "50"), ("include_private", "false")],
    )
    .await
    .unwrap_or(Value::Null);
    result["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["name"].as_str().map(str::to_owned))
        .collect()
}

/// `reconciler/ckan.rs` `sweep_with`'s search: the platform-made datasets of `organization`.
fn sweep_names(api: &mut HttpCkan, organization: &str) -> Vec<String> {
    let page = api
        .action(
            "package_search",
            &json!({
                "fq": format!("organization:\"{organization}\""),
                "include_private": true,
                "rows": 1000,
                "start": 0,
            }),
        )
        .expect("the sweep search");
    assert!(page["count"].as_u64().is_some(), "{page}");
    page["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| {
            d["organization"]["name"] == json!(organization)
                && d["extras"].as_array().is_some_and(|extras| {
                    extras
                        .iter()
                        .any(|e| e["key"] == "generated_by" && e["value"] == GENERATOR)
                })
        })
        .filter_map(|d| d["name"].as_str().map(str::to_owned))
        .collect()
}

/// Runs a blocking Portal call off the async runtime, as the reconciler's `spawn_blocking` does.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("the call ran")
}

const PUBLIC_SLUG: &str = "zt4qm7ge2xdv6ksb3ncf5arw2y";
const CLOSED_SLUG: &str = "ab4qm7ge2xdv6ksb3ncf5arw2y";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_portal_publisher_converges_and_withdraws_against_dataroom() {
    let base = serve().await;

    // publish (jcctl ckan.rs `publish`): organization_show 404 -> organization_create,
    // package_show 404 -> package_create.
    let b = base.clone();
    let (first, again) = blocking(move || {
        let mut api = client(&b, TOKEN);
        let ep = endpoint("ovzdusie-public", PUBLIC_DATASET, "public", PUBLIC_SLUG);
        let first = publish(
            &mut api,
            &ep,
            &instance(),
            &record("Kvalita ovzdušia"),
            &settings("Mesto Banská Bystrica"),
        );
        // A second run over an unchanged Endpoint writes nothing (CC-18): every field the
        // publisher compares came back as it was sent.
        let again = publish(
            &mut api,
            &ep,
            &instance(),
            &record("Kvalita ovzdušia"),
            &settings("Mesto Banská Bystrica"),
        );
        (first, again)
    })
    .await;
    assert_eq!(first.expect("created"), Outcome::Created);
    assert_eq!(again.expect("converged"), Outcome::Unchanged);

    // EP-139: a public Endpoint's dataset is read anonymously, with the fields the Portal reads.
    let shown = portal_dataset(&base, PUBLIC_DATASET)
        .await
        .expect("the public dataset is visible anonymously");
    assert_eq!(shown["organization"]["name"], ORGANIZATION);
    assert_eq!(shown["organization"]["title"], "Mesto Banská Bystrica");
    assert_eq!(shown["title"], "Kvalita ovzdušia (en)");
    assert_eq!(shown["notes"], "Hourly air quality observations.");
    let urls: Vec<&str> = shown["resources"]
        .as_array()
        .expect("resources")
        .iter()
        .filter_map(|r| r["url"].as_str())
        .collect();
    assert!(
        urls.iter().any(|u| u.starts_with(&format!(
            "https://data.city.example/api/endpoint/{PUBLIC_SLUG}"
        ))),
        "{urls:?}"
    );
    assert!(
        shown["resources"]
            .as_array()
            .expect("resources")
            .iter()
            .all(|r| r["format"].is_string()),
        "{shown}"
    );
    let endpoint_extra = shown["extras"]
        .as_array()
        .expect("extras")
        .iter()
        .find(|e| e["key"] == "endpoint")
        .and_then(|e| e["value"].as_str())
        .unwrap_or_default()
        .to_owned();
    assert!(endpoint_extra.contains(PUBLIC_SLUG), "{shown}");
    assert_eq!(portal_search(&base, "ovzdusia").await, [PUBLIC_DATASET]);

    // publish_views (EP-132): resource_view_list, resource_view_create; a rerun writes nothing.
    let b = base.clone();
    let (views, views_again) = blocking(move || {
        let mut api = client(&b, TOKEN);
        let ep = endpoint("ovzdusie-public", PUBLIC_DATASET, "public", PUBLIC_SLUG);
        let s = settings("Mesto Banská Bystrica");
        (
            publish_views(&mut api, &ep, &s, &[]),
            publish_views(&mut api, &ep, &s, &[]),
        )
    })
    .await;
    let views = views.expect("views published");
    assert_eq!((views.created, views.updated, views.deleted), (1, 0, 0));
    let views_again = views_again.expect("views converged");
    assert_eq!(
        (
            views_again.created,
            views_again.updated,
            views_again.deleted
        ),
        (0, 0, 0)
    );

    // EP-139: a closed Endpoint's dataset is private: the Portal's token reads it, nobody else.
    let b = base.clone();
    let closed = blocking(move || {
        let mut api = client(&b, TOKEN);
        let ep = endpoint(
            "ovzdusie-interna",
            CLOSED_DATASET,
            "organization",
            CLOSED_SLUG,
        );
        publish(
            &mut api,
            &ep,
            &instance(),
            &record("Interná"),
            &settings("Mesto Banská Bystrica"),
        )
    })
    .await;
    assert_eq!(closed.expect("created"), Outcome::Created);
    assert!(portal_dataset(&base, CLOSED_DATASET).await.is_none());
    assert!(anonymous(&base, "package_show", &[("id", CLOSED_DATASET)])
        .await
        .is_none());
    assert!(!portal_search(&base, "")
        .await
        .contains(&CLOSED_DATASET.to_owned()));
    let b = base.clone();
    let (held, mut swept) = blocking(move || {
        let mut api = client(&b, TOKEN);
        let held = api.show("package_show", CLOSED_DATASET).expect("show");
        (held, sweep_names(&mut api, ORGANIZATION))
    })
    .await;
    assert_eq!(held.expect("the token reads it")["private"], json!(true));
    swept.sort();
    assert_eq!(swept, [PUBLIC_DATASET, CLOSED_DATASET]);

    // `reconciler/catalog.rs` `packages`: the catalog wave pages `package_search` anonymously,
    // sorted by name, and folds each answer with jc-core `Package::from_ckan`.
    let page = anonymous(
        &base,
        "package_search",
        &[("rows", "1000"), ("start", "0"), ("sort", "name asc")],
    )
    .await
    .expect("a package_search result");
    assert_eq!(page["count"], json!(1), "only the public dataset: {page}");
    let packages: Vec<Package> = page["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(Package::from_ckan)
        .collect();
    assert_eq!(packages.len(), 1, "{page}");
    assert_eq!(packages[0].name, PUBLIC_DATASET);
    assert_eq!(
        packages[0].organization.as_ref().map(|o| o.name.as_str()),
        Some(ORGANIZATION)
    );

    // An Endpoint that stops being public takes its dataset out of anonymous sight.
    let b = base.clone();
    let narrowed = blocking(move || {
        let mut api = client(&b, TOKEN);
        let ep = endpoint(
            "ovzdusie-public",
            PUBLIC_DATASET,
            "organization",
            PUBLIC_SLUG,
        );
        publish(
            &mut api,
            &ep,
            &instance(),
            &record("Kvalita ovzdušia"),
            &settings("Mesto Banská Bystrica"),
        )
    })
    .await;
    assert_eq!(narrowed.expect("updated"), Outcome::Updated);
    assert!(portal_dataset(&base, PUBLIC_DATASET).await.is_none());
    let b = base.clone();
    let widened = blocking(move || {
        let mut api = client(&b, TOKEN);
        let ep = endpoint("ovzdusie-public", PUBLIC_DATASET, "public", PUBLIC_SLUG);
        publish(
            &mut api,
            &ep,
            &instance(),
            &record("Kvalita ovzdušia"),
            &settings("Mesto Banská Bystrica"),
        )
    })
    .await;
    assert_eq!(widened.expect("updated"), Outcome::Updated);
    assert!(portal_dataset(&base, PUBLIC_DATASET).await.is_some());

    // The instance's declared title renames the organization (organization_patch).
    let b = base.clone();
    let renamed = blocking(move || {
        let mut api = client(&b, TOKEN);
        let ep = endpoint("ovzdusie-public", PUBLIC_DATASET, "public", PUBLIC_SLUG);
        publish(
            &mut api,
            &ep,
            &instance(),
            &record("Kvalita ovzdušia"),
            &settings("Mesto BB"),
        )
    })
    .await;
    assert_eq!(renamed.expect("converged"), Outcome::Unchanged);
    let shown = portal_dataset(&base, PUBLIC_DATASET)
        .await
        .expect("visible");
    assert_eq!(shown["organization"]["title"], "Mesto BB");

    // withdraw (jcctl ckan.rs `withdraw`): package_show, package_delete; gone for everyone
    // reading anonymously, "not there" to a second withdraw, and back on republish.
    let b = base.clone();
    let (gone, gone_again) = blocking(move || {
        let mut api = client(&b, TOKEN);
        (
            withdraw(&mut api, PUBLIC_DATASET),
            withdraw(&mut api, PUBLIC_DATASET),
        )
    })
    .await;
    assert_eq!(gone.expect("withdrawn"), Outcome::Withdrawn);
    assert_eq!(gone_again.expect("idempotent"), Outcome::Unchanged);
    assert!(portal_dataset(&base, PUBLIC_DATASET).await.is_none());
    assert!(portal_search(&base, "ovzdusia").await.is_empty());
    let b = base.clone();
    let back = blocking(move || {
        let mut api = client(&b, TOKEN);
        let ep = endpoint("ovzdusie-public", PUBLIC_DATASET, "public", PUBLIC_SLUG);
        publish(
            &mut api,
            &ep,
            &instance(),
            &record("Kvalita ovzdušia"),
            &settings("Mesto BB"),
        )
    })
    .await;
    assert_eq!(back.expect("republished"), Outcome::Updated);
    assert!(portal_dataset(&base, PUBLIC_DATASET).await.is_some());
}

/// ADR-N-068 (EP-175, EP-176, EP-178): one public Endpoint listing two datasets in
/// `spec.catalog.datasets` is two datasets, each narrowed to its slice; both read anonymously and
/// fold into the NGSI-LD catalogue by the Endpoint's slug; a rerun writes nothing; withdrawing
/// one leaves the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn several_datasets_of_one_endpoint_converge_and_withdraw_one_by_one() {
    let base = serve().await;
    let manifest = || -> RawManifest {
        let mut ep = endpoint("ovzdusie-public", PUBLIC_DATASET, "public", PUBLIC_SLUG);
        ep.spec["catalog"]["datasets"] = json!([
            { "name": "bb-pm", "title": { "sk": "Prachové častice" },
              "description": { "sk": "PM10 a PM2.5" }, "types": ["AirQualityObserved"] },
            { "name": "bb-zle-ovzdusie", "title": { "sk": "Zlé ovzdušie" },
              "description": { "sk": "Nad limitom" }, "types": ["AirQualityObserved"],
              "q": "pm10>50" },
        ]);
        ep.spec.as_object_mut().expect("a spec").remove("publish");
        ep
    };
    let run = |base: String| {
        blocking(move || {
            let mut api = client(&base, TOKEN);
            let ep = manifest();
            let spec: jc_core::kinds::EndpointSpec =
                serde_json::from_value(ep.spec.clone()).expect("the spec");
            let publication =
                jcctl::publish::ckan::publication_of(&spec).expect("a public Endpoint publishes");
            let slices = spec.catalog.expect("a catalogue block").datasets;
            slices
                .iter()
                .map(|slice| {
                    publish_dataset(
                        &mut api,
                        &ep,
                        &publication,
                        Some(slice),
                        &instance(),
                        &record("Kvalita ovzdušia"),
                        &settings("Mesto Banská Bystrica"),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
        })
    };
    let first = run(base.clone()).await.expect("published");
    assert_eq!(first, [Outcome::Created, Outcome::Created]);
    let again = run(base.clone()).await.expect("converged");
    assert_eq!(again, [Outcome::Unchanged, Outcome::Unchanged]);

    let zle = portal_dataset(&base, "bb-zle-ovzdusie")
        .await
        .expect("the second dataset reads anonymously");
    assert_eq!(
        zle["url"],
        json!(format!(
            "https://data.city.example/api/endpoint/{PUBLIC_SLUG}/datasets/bb-zle-ovzdusie"
        ))
    );
    let page = anonymous(
        &base,
        "package_search",
        &[("rows", "1000"), ("start", "0"), ("sort", "name asc")],
    )
    .await
    .expect("a package_search result");
    let packages: Vec<Package> = page["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(Package::from_ckan)
        .collect();
    // A package as the catalogue reads it: name, the Endpoint's slug, the (type, q) of its rows.
    type Read<'a> = (&'a str, &'a str, Option<(String, Option<String>)>);
    let folded: Vec<Read> = packages
        .iter()
        .map(|p| (p.name.as_str(), p.slug.as_str(), p.rows.clone()))
        .collect();
    assert_eq!(
        folded,
        [
            (
                "bb-pm",
                PUBLIC_SLUG,
                Some(("AirQualityObserved".to_owned(), None))
            ),
            (
                "bb-zle-ovzdusie",
                PUBLIC_SLUG,
                Some(("AirQualityObserved".to_owned(), Some("pm10>50".to_owned())))
            ),
        ],
        "{page}"
    );

    let b = base.clone();
    let gone = blocking(move || withdraw(&mut client(&b, TOKEN), "bb-zle-ovzdusie")).await;
    assert_eq!(gone.expect("withdrawn"), Outcome::Withdrawn);
    assert!(portal_dataset(&base, "bb-zle-ovzdusie").await.is_none());
    assert!(portal_dataset(&base, "bb-pm").await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_portal_publisher_without_a_valid_token_writes_nothing() {
    let base = serve().await;
    let b = base.clone();
    let (wrong, anon_write) = blocking(move || {
        let mut api = client(&b, "not-the-token");
        let ep = endpoint("ovzdusie-public", PUBLIC_DATASET, "public", PUBLIC_SLUG);
        let wrong = publish(&mut api, &ep, &instance(), &record("X"), &settings("X"));
        let anon_write = reqwest::blocking::Client::new()
            .post(format!("{b}/api/3/action/package_create"))
            .json(&json!({ "name": "anonymous", "owner_org": ORGANIZATION }))
            .send()
            .expect("answer");
        let status = anon_write.status();
        let body: Value = anon_write.json().expect("a CKAN envelope");
        (wrong, (status, body))
    })
    .await;
    // HttpCkan reads a 401/403 envelope as a refusal that names no token.
    match wrong {
        Err(PublishError::Api(CkanError::Rejected { message, .. })) => {
            assert!(!message.contains("not-the-token"), "{message}");
        }
        other => panic!("a wrong token must be refused, got {other:?}"),
    }
    let (status, body) = anon_write;
    assert!(status.is_client_error(), "{status}");
    assert_eq!(body["success"], json!(false));
    assert!(body["error"]["__type"].is_string(), "{body}");
    assert!(anonymous(&base, "package_show", &[("id", "anonymous")])
        .await
        .is_none());
}

#[test]
fn the_portal_publisher_token_is_long_read_from_its_file_and_never_echoed() {
    let org_id = OrgId::from_uuid(Uuid::from_u128(1));
    let cat_id = CatalogueId::from_uuid(Uuid::from_u128(2));
    let short = "x".repeat(MIN_PORTAL_TOKEN_LEN - 1);
    let refused = portal_publisher(&short, org_id, cat_id).expect_err("too short");
    assert!(!refused.to_string().contains(&short), "{refused}");
    let spaced = format!("{} {}", "a".repeat(20), "b".repeat(20));
    assert!(portal_publisher(&spaced, org_id, cat_id).is_err());

    let dir = std::env::temp_dir().join(format!("dataroom-portal-token-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("token");
    std::fs::write(&file, format!("{TOKEN}\n")).expect("write");
    let (binding, grant) = portal_publisher_from_file(&file, org_id, cat_id).expect("read");
    assert!(
        !format!("{binding:?}").contains(TOKEN),
        "the token must not print"
    );
    assert_eq!(binding.token, TOKEN);
    assert_eq!(grant.actions, [Action::Read, Action::Propose]);
    assert_eq!(grant.catalogue_id, Some(cat_id));
    let missing =
        portal_publisher_from_file(&dir.join("none"), org_id, cat_id).expect_err("absent");
    assert!(
        missing.to_string().contains("could not be read"),
        "{missing}"
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
