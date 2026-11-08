#![forbid(unsafe_code)]
//! Harvest runs of every source kind through `HarvestEngine::run_source`: what is written, what
//! is withdrawn, and that nothing is withdrawn or overwritten unless the read was complete and
//! the record belongs to the source (C9).

mod common;

use common::*;
use dataroom_core::ops::HarvestRunner;
use dataroom_core::{DataroomError, HarvestRunState, RecordState, Store};
use dataroom_harvest::MapFetcher;
use serde_json::json;

const HUB: &str = "https://hub.city.example";
const HUB_PAGE1: &str =
    "https://hub.city.example/api/search/v1/collections/dataset/items?limit=100";

#[tokio::test]
async fn arcgis_hub_reads_every_page_and_keeps_unchanged_items() {
    let w = world(&[]).await;
    let src = source("arc-gis-hub", "hub", HUB, &["hub.city.example"]);
    let f = MapFetcher::new();
    f.insert(
        HUB_PAGE1,
        Some("application/json"),
        json!({"numberMatched": 3, "features": [hub_item("aaa111", "Cesty", 1)],
               "links": [{"rel": "self", "href": HUB_PAGE1},
                         {"rel": "next", "href": format!("{HUB}/page2")}]})
        .to_string(),
    );
    f.insert(
        format!("{HUB}/page2"),
        Some("application/json"),
        json!({"features": [hub_item("bbb222", "Školy", 1), hub_item("ccc333", "Parky", 1)],
               "links": []})
        .to_string(),
    );
    let e = engine(&f);
    let run = e.run_source(&w.ctx, &w.store, &src).await.expect("run");
    assert_eq!(
        (run.created_count, run.state),
        (3, HarvestRunState::Succeeded)
    );
    assert_eq!(active_names(&w).await.len(), 3);
    let again = e.run_source(&w.ctx, &w.store, &src).await.expect("run 2");
    assert_eq!((again.created_count, again.unchanged_count), (0, 3));
    let runs = w.store.list_harvest_runs(&w.ctx, None).await.unwrap();
    assert_eq!(runs.len(), 2);
    let act = w
        .store
        .list_activity(&w.ctx, Some("hs-hub"), 10)
        .await
        .unwrap();
    assert_eq!(act.len(), 2);
    assert!(act[0].summary.contains("unchanged 3"), "{}", act[0].summary);
}

#[tokio::test]
async fn arcgis_hub_without_a_match_count_is_an_incomplete_read() {
    let w = world(&[]).await;
    let src = source("arc-gis-hub", "hub", HUB, &["hub.city.example"]);
    let f = MapFetcher::new();
    let full = json!({"numberMatched": 2,
        "features": [hub_item("aaa111", "Cesty", 1), hub_item("bbb222", "Školy", 1)]});
    f.insert(HUB_PAGE1, Some("application/json"), full.to_string());
    let e = engine(&f);
    e.run_source(&w.ctx, &w.store, &src)
        .await
        .expect("first run");

    // The Hub now answers one item and no count: the read cannot be shown complete, so nothing
    // is withdrawn (harvest-hub.py: `matched is None or len(items) != matched`).
    f.insert(
        HUB_PAGE1,
        Some("application/json"),
        json!({"features": [hub_item("aaa111", "Cesty", 1)]}).to_string(),
    );
    let err = e.run_source(&w.ctx, &w.store, &src).await.unwrap_err();
    assert!(err.to_string().contains("incomplete"), "{err}");
    assert_eq!(active_names(&w).await.len(), 2, "nothing withdrawn");
    let runs = w.store.list_harvest_runs(&w.ctx, None).await.unwrap();
    assert_eq!(runs[0].state, HarvestRunState::Failed);
    assert!(runs[0]
        .error
        .as_deref()
        .unwrap_or("")
        .contains("incomplete"));

    // A count that disagrees with what was read is refused the same way.
    f.insert(
        HUB_PAGE1,
        Some("application/json"),
        json!({"numberMatched": 5, "features": [hub_item("aaa111", "Cesty", 1)]}).to_string(),
    );
    let err = e.run_source(&w.ctx, &w.store, &src).await.unwrap_err();
    assert!(err.to_string().contains("1 of 5"), "{err}");
    assert_eq!(active_names(&w).await.len(), 2);
}

#[tokio::test]
async fn a_hub_that_keeps_paging_past_the_page_cap_withdraws_nothing() {
    let w = world(&[]).await;
    let src = source("arc-gis-hub", "hub", HUB, &["hub.city.example"]);
    let f = MapFetcher::new();
    f.insert(
        HUB_PAGE1,
        Some("application/json"),
        json!({"numberMatched": 1, "features": [hub_item("aaa111", "Cesty", 1)]}).to_string(),
    );
    let e = engine(&f);
    e.run_source(&w.ctx, &w.store, &src).await.unwrap();
    // Every page links to the next one, forever.
    f.insert(
        HUB_PAGE1,
        Some("application/json"),
        json!({"numberMatched": 1000, "features": [],
               "links": [{"rel": "next", "href": format!("{HUB}/p/1")}]})
        .to_string(),
    );
    for i in 1..60 {
        f.insert(
            format!("{HUB}/p/{i}"),
            Some("application/json"),
            json!({"features": [], "links": [{"rel": "next", "href": format!("{HUB}/p/{}", i + 1)}]})
                .to_string(),
        );
    }
    let err = e.run_source(&w.ctx, &w.store, &src).await.unwrap_err();
    assert!(err.to_string().contains("incomplete"), "{err}");
    assert_eq!(active_names(&w).await.len(), 1);
}

#[tokio::test]
async fn arcgis_hub_answers_that_are_not_json_fail_the_run() {
    let w = world(&[]).await;
    let src = source("arc-gis-hub", "hub", HUB, &["hub.city.example"]);
    let f = MapFetcher::new();
    f.insert(HUB_PAGE1, Some("text/html"), "<html>maintenance</html>");
    let err = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid Hub JSON"), "{err}");
    let runs = w.store.list_harvest_runs(&w.ctx, None).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].logs[0].fingerprint,
        "harvest.arcgis-hub.fetch_error"
    );
}

#[tokio::test]
async fn a_source_with_nothing_to_keep_records_a_failed_run_and_withdraws_nothing() {
    let w = world(&[]).await;
    let src = source("arc-gis-hub", "hub", HUB, &["hub.city.example"]);
    let f = MapFetcher::new();
    f.insert(
        HUB_PAGE1,
        Some("application/json"),
        json!({"numberMatched": 1, "features": [hub_item("aaa111", "Cesty", 1)]}).to_string(),
    );
    let e = engine(&f);
    e.run_source(&w.ctx, &w.store, &src).await.unwrap();
    f.insert(
        HUB_PAGE1,
        Some("application/json"),
        json!({"numberMatched": 0, "features": []}).to_string(),
    );
    let err = e.run_source(&w.ctx, &w.store, &src).await.unwrap_err();
    assert!(err.to_string().contains("0 items to keep"), "{err}");
    assert_eq!(active_names(&w).await.len(), 1);
    let runs = w.store.list_harvest_runs(&w.ctx, None).await.unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(
        runs[0].state,
        HarvestRunState::Failed,
        "the outage is on record"
    );
}

#[tokio::test]
async fn nkod_sparql_run_creates_datasets_with_open_licences() {
    let w = world(&[]).await;
    let url = "https://data.gov.cz/sparql?query=x";
    let src = source(
        "nkod-sparql",
        "nkod",
        url,
        &["data.gov.cz", "files.brno.cz"],
    );
    let f = MapFetcher::new();
    let cell = |v: &str| json!({"type": "uri", "value": v});
    f.insert(
        url,
        Some("application/sparql-results+json"),
        json!({"results": {"bindings": [{
            "ds": cell("https://data.gov.cz/zdroj/datové-sady/1"),
            "title": cell("Kvalita ovzduší"),
            "dist": cell("https://data.gov.cz/zdroj/distribuce/1"),
            "url": cell("https://files.brno.cz/air.csv"),
            "work": cell("https://creativecommons.org/licenses/by/4.0/")
        }]}})
        .to_string(),
    );
    let run = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .expect("nkod run");
    assert_eq!(run.created_count, 1);
    assert_eq!(active_names(&w).await, ["kvalita-ovzdusi"]);

    f.insert(url, Some("application/json"), "{not json");
    let err = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid SPARQL JSON"), "{err}");
}

#[tokio::test]
async fn dcat_feed_follows_hydra_pages_and_withdraws_what_a_complete_read_drops() {
    let w = world(&[]).await;
    let p1 = "https://feeds.example.org/catalog.ttl";
    let p2 = "https://feeds.example.org/catalog.ttl?page=2";
    let src = source("dcat-ap", "feed", p1, &["feeds.example.org"]);
    let f = MapFetcher::new();
    f.insert(
        p1,
        Some("text/turtle"),
        dcat_turtle(
            &[("https://feeds.example.org/ds/air-quality", "Ovzdušie")],
            Some(p2),
        ),
    );
    f.insert(
        p2,
        Some("text/turtle"),
        dcat_turtle(&[("https://feeds.example.org/ds/noise-map", "Hluk")], None),
    );
    let e = engine(&f);
    let run = e
        .run_source(&w.ctx, &w.store, &src)
        .await
        .expect("dcat run");
    assert_eq!(run.created_count, 2);
    assert_eq!(active_names(&w).await, ["air-quality", "noise-map"]);

    f.insert(
        p1,
        Some("text/turtle"),
        dcat_turtle(
            &[("https://feeds.example.org/ds/air-quality", "Ovzdušie 2")],
            None,
        ),
    );
    let run = e
        .run_source(&w.ctx, &w.store, &src)
        .await
        .expect("dcat run 2");
    assert_eq!((run.updated_count, run.withdrawn_count), (1, 1));
    assert_eq!(active_names(&w).await, ["air-quality"]);
    assert!(run
        .logs
        .iter()
        .any(|l| l.fingerprint == "harvest.withdrawn"));
}

#[tokio::test]
async fn dcat_feed_whose_pages_loop_withdraws_nothing() {
    let w = world(&[]).await;
    let p1 = "https://feeds.example.org/catalog.ttl";
    let p2 = "https://feeds.example.org/catalog.ttl?page=2";
    let src = source("dcat-ap", "feed", p1, &["feeds.example.org"]);
    let f = MapFetcher::new();
    f.insert(
        p1,
        Some("text/turtle"),
        dcat_turtle(
            &[
                ("https://feeds.example.org/ds/air-quality", "Ovzdušie"),
                ("https://feeds.example.org/ds/noise-map", "Hluk"),
            ],
            None,
        ),
    );
    let e = engine(&f);
    e.run_source(&w.ctx, &w.store, &src).await.unwrap();
    // Page 2 points back to page 1: the listing never ends, so it is no complete read.
    f.insert(
        p1,
        Some("text/turtle"),
        dcat_turtle(
            &[("https://feeds.example.org/ds/air-quality", "Ovzdušie")],
            Some(p2),
        ),
    );
    f.insert(p2, Some("text/turtle"), dcat_turtle(&[], Some(p1)));
    let err = e.run_source(&w.ctx, &w.store, &src).await.unwrap_err();
    assert!(err.to_string().contains("incomplete"), "{err}");
    assert_eq!(active_names(&w).await, ["air-quality", "noise-map"]);
}

#[tokio::test]
async fn dcat_feed_that_does_not_parse_fails_the_run() {
    let w = world(&[]).await;
    let p1 = "https://feeds.example.org/catalog.ttl";
    let src = source("dcat-ap", "feed", p1, &["feeds.example.org"]);
    let f = MapFetcher::new();
    f.insert(p1, Some("text/turtle"), "<<< not turtle");
    let err = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .unwrap_err();
    assert!(matches!(err, DataroomError::Rdf(_)), "{err:?}");
}

#[tokio::test]
async fn ckan_source_reads_every_page_of_package_search() {
    let w = world(&[]).await;
    let base = "https://ckan.remote.example";
    let src = source("ckan-action", "remote", base, &["ckan.remote.example"]);
    let f = MapFetcher::new();
    f.insert(
        format!("{base}/api/3/action/package_search?rows=1000&start=0"),
        Some("application/json"),
        ckan_page(2, vec![ckan_package("r-1", "remote-air", "Air")]),
    );
    f.insert(
        format!("{base}/api/3/action/package_search?rows=1000&start=1"),
        Some("application/json"),
        ckan_page(2, vec![ckan_package("r-2", "remote-noise", "Noise")]),
    );
    let run = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .expect("ckan run");
    assert_eq!(run.created_count, 2);
    let all = w.store.list_datasets(&w.ctx).await.unwrap();
    assert!(all.iter().all(|d| d.publisher_id == "city"));
    assert!(all
        .iter()
        .all(|d| d.harvest_source_id.as_deref() == Some("remote")));
}

#[tokio::test]
async fn ckan_source_url_with_a_query_keeps_its_filter() {
    let w = world(&[]).await;
    let base = "https://ckan.remote.example/api/3/action/package_search?fq=organization:city";
    let src = source("ckan-action", "remote", base, &["ckan.remote.example"]);
    let f = MapFetcher::new();
    f.insert(
        format!("{base}&rows=1000&start=0"),
        Some("application/json"),
        ckan_page(1, vec![ckan_package("r-1", "remote-air", "Air")]),
    );
    let run = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .expect("ckan run with fq");
    assert_eq!(run.created_count, 1);
}

#[tokio::test]
async fn ckan_source_that_stops_short_of_its_count_withdraws_nothing() {
    let w = world(&[]).await;
    let base = "https://ckan.remote.example";
    let src = source("ckan-action", "remote", base, &["ckan.remote.example"]);
    let f = MapFetcher::new();
    let first = format!("{base}/api/3/action/package_search?rows=1000&start=0");
    f.insert(
        &first,
        Some("application/json"),
        ckan_page(
            2,
            vec![
                ckan_package("r-1", "remote-air", "Air"),
                ckan_package("r-2", "remote-noise", "Noise"),
            ],
        ),
    );
    let e = engine(&f);
    e.run_source(&w.ctx, &w.store, &src).await.unwrap();
    // The source claims 2 records but the next page is empty: an incomplete read.
    f.insert(
        &first,
        Some("application/json"),
        ckan_page(2, vec![ckan_package("r-1", "remote-air", "Air")]),
    );
    f.insert(
        format!("{base}/api/3/action/package_search?rows=1000&start=1"),
        Some("application/json"),
        ckan_page(2, vec![]),
    );
    let err = e.run_source(&w.ctx, &w.store, &src).await.unwrap_err();
    assert!(err.to_string().contains("incomplete"), "{err}");
    assert_eq!(active_names(&w).await, ["remote-air", "remote-noise"]);

    f.insert(
        &first,
        Some("application/json"),
        json!({"success": false}).to_string(),
    );
    let err = e.run_source(&w.ctx, &w.store, &src).await.unwrap_err();
    assert!(err.to_string().contains("success=false"), "{err}");
}

#[tokio::test]
async fn a_harvested_record_never_overwrites_a_local_dataset() {
    let w = world(&[]).await;
    // A dataset the catalogue manages itself.
    w.store
        .upsert_dataset(&w.ctx, local_dataset("local-1", "budget", "Rozpočet"))
        .await
        .unwrap();
    let base = "https://ckan.remote.example";
    let src = source("ckan-action", "remote", base, &["ckan.remote.example"]);
    let f = MapFetcher::new();
    // The remote CKAN answers a package carrying the local dataset's id, and one its name.
    f.insert(
        format!("{base}/api/3/action/package_search?rows=1000&start=0"),
        Some("application/json"),
        ckan_page(
            3,
            vec![
                ckan_package("local-1", "remote-air", "Hijacked"),
                ckan_package("r-2", "budget", "Hijacked by name"),
                ckan_package("r-3", "remote-noise", "Noise"),
            ],
        ),
    );
    let run = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .expect("run");
    let local = w
        .store
        .get_dataset(&w.ctx, "budget")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(local.id, "local-1");
    assert_eq!(
        local.title["sk"], "Rozpočet",
        "the local dataset is untouched"
    );
    assert!(local.harvest_source_id.is_none());
    assert_eq!(
        run.skipped.len(),
        1,
        "the name clash is skipped: {:?}",
        run.skipped
    );
    assert!(run.skipped[0].reason.contains("budget"));
    assert_eq!(run.created_count, 2);
}

#[tokio::test]
async fn two_sources_listing_one_record_do_not_take_it_from_each_other() {
    let w = world(&[]).await;
    let iri = "https://feeds.example.org/ds/air-quality";
    let a = source(
        "dcat-ap",
        "feed-a",
        "https://feeds.example.org/a.ttl",
        &["feeds.example.org"],
    );
    let b = source(
        "dcat-ap",
        "feed-b",
        "https://feeds.example.org/b.ttl",
        &["feeds.example.org"],
    );
    let f = MapFetcher::new();
    f.insert(
        &a.url,
        Some("text/turtle"),
        dcat_turtle(&[(iri, "Ovzdušie")], None),
    );
    f.insert(
        &b.url,
        Some("text/turtle"),
        dcat_turtle(
            &[
                (iri, "Ovzdušie z B"),
                ("https://feeds.example.org/ds/noise-map", "Hluk"),
            ],
            None,
        ),
    );
    let e = engine(&f);
    e.run_source(&w.ctx, &w.store, &a).await.unwrap();
    let run_b = e.run_source(&w.ctx, &w.store, &b).await.unwrap();
    assert_eq!(run_b.created_count, 1);
    assert_eq!(run_b.skipped.len(), 1);
    let held = w
        .store
        .get_dataset(&w.ctx, "air-quality")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.harvest_source_id.as_deref(), Some("feed-a"));
    assert_eq!(held.title["sk"], "Ovzdušie");
}

#[tokio::test]
async fn a_withdrawn_harvested_record_comes_back_when_the_source_lists_it_again() {
    let w = world(&[]).await;
    let p1 = "https://feeds.example.org/catalog.ttl";
    let src = source("dcat-ap", "feed", p1, &["feeds.example.org"]);
    let f = MapFetcher::new();
    let both = dcat_turtle(
        &[
            ("https://feeds.example.org/ds/air-quality", "Ovzdušie"),
            ("https://feeds.example.org/ds/noise-map", "Hluk"),
        ],
        None,
    );
    f.insert(p1, Some("text/turtle"), both.clone());
    let e = engine(&f);
    e.run_source(&w.ctx, &w.store, &src).await.unwrap();
    f.insert(
        p1,
        Some("text/turtle"),
        dcat_turtle(
            &[("https://feeds.example.org/ds/air-quality", "Ovzdušie")],
            None,
        ),
    );
    e.run_source(&w.ctx, &w.store, &src).await.unwrap();
    assert_eq!(active_names(&w).await, ["air-quality"]);
    f.insert(p1, Some("text/turtle"), both);
    let run = e.run_source(&w.ctx, &w.store, &src).await.unwrap();
    assert_eq!((run.updated_count, run.unchanged_count), (1, 1));
    assert_eq!(active_names(&w).await, ["air-quality", "noise-map"]);
    let ds = w
        .store
        .get_dataset(&w.ctx, "noise-map")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ds.state, RecordState::Active);
}

#[tokio::test]
async fn a_run_needs_its_catalogue_and_its_publisher() {
    let w = world(&[]).await;
    let f = MapFetcher::new();
    let e = engine(&f);
    let mut src = source(
        "dcat-ap",
        "feed",
        "https://feeds.example.org/c.ttl",
        &["feeds.example.org"],
    );
    src.publisher_id = "nobody".into();
    let err = e.run_source(&w.ctx, &w.store, &src).await.unwrap_err();
    assert!(matches!(err, DataroomError::Validation { .. }), "{err:?}");

    let elsewhere = dataroom_core::RequestCtx::new(
        w.org,
        dataroom_core::CatalogueId::from_uuid(uuid::Uuid::now_v7()),
        dataroom_core::Subject::authenticated("harvester", vec![w.org], Vec::new()),
    );
    let err = e.run_source(&elsewhere, &w.store, &src).await.unwrap_err();
    assert!(matches!(err, DataroomError::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn harvesting_a_catalogue_that_blocks_other_organizations_is_forbidden() {
    let w = world(&[]).await;
    // Another organization's catalogue on this installation, closed to harvesters.
    let other_org = dataroom_core::OrgId::from_uuid(uuid::Uuid::now_v7());
    let other_cat = dataroom_core::CatalogueId::from_uuid(uuid::Uuid::now_v7());
    let mut closed = catalogue(other_org, other_cat, "closed.other.example", &[]);
    closed.allow_cross_org_harvest = false;
    w.store.upsert_catalogue(closed).await.unwrap();
    let src = source(
        "dcat-ap",
        "neighbour",
        "https://closed.other.example/catalog.ttl",
        &["closed.other.example"],
    );
    let f = MapFetcher::new();
    f.insert(
        &src.url,
        Some("text/turtle"),
        dcat_turtle(&[("https://closed.other.example/ds/x", "X")], None),
    );
    let err = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .unwrap_err();
    assert!(matches!(err, DataroomError::Forbidden(_)), "{err:?}");
    assert!(f.pinned_seen().is_empty(), "nothing was fetched");

    // The same catalogue open to harvesting is read.
    let mut open = catalogue(other_org, other_cat, "closed.other.example", &[]);
    open.allow_cross_org_harvest = true;
    w.store.upsert_catalogue(open).await.unwrap();
    let run = engine(&f).run_source(&w.ctx, &w.store, &src).await.unwrap();
    assert_eq!(run.created_count, 1);
}

#[tokio::test]
async fn the_catalogue_outbound_list_narrows_every_source() {
    let w = world(&["other.example.org"]).await;
    let src = source(
        "dcat-ap",
        "feed",
        "https://feeds.example.org/c.ttl",
        &["feeds.example.org"],
    );
    let f = MapFetcher::new();
    f.insert(
        &src.url,
        Some("text/turtle"),
        dcat_turtle(&[("https://feeds.example.org/ds/x", "X")], None),
    );
    let err = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("allowed_outbound_hosts"), "{err}");
    assert!(f.pinned_seen().is_empty());
}
