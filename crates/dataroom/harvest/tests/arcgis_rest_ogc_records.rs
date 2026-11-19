#![forbid(unsafe_code)]
//! The ArcGIS REST and OGC API Records sources (T-7427) on answers recorded from public servers
//! (`tests/fixtures/README.md`): what a record holds, paging, skips with their reason, the
//! budget, a changed-only re-run, and the guard on every request.

mod common;

use common::*;
use dataroom_core::ops::{HarvestRunner, SourceTest};
use dataroom_core::{
    CatalogueId, Dataset, HarvestBudget, HarvestRunState, HarvestSource, HarvestSourceKind, OrgId,
    Store,
};
use dataroom_harvest::sources::{
    arcgis_extent_wgs84, arcgis_services_root, open_licence, parse_arcgis_directory,
    parse_arcgis_service, parse_ogc_records_page,
};
use dataroom_harvest::{HarvestEngine, MapFetcher};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use uuid::Uuid;

const GIS: &str = "https://sampleserver6.arcgisonline.com/arcgis/rest/services";
const RECORDS: &str = "https://demo.pygeoapi.io/master/collections/dutch-metadata";

fn fixture(path: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect(path)
}

fn gis_source() -> HarvestSource {
    source(
        "arcgis-rest",
        "gis",
        GIS,
        &["sampleserver6.arcgisonline.com"],
    )
}

fn records_source() -> HarvestSource {
    source("ogc-records", "ngr", RECORDS, &["demo.pygeoapi.io"])
}

/// The sample server: the root directory, its `Elevation` folder and three services.
fn gis_fetcher() -> MapFetcher {
    let f = MapFetcher::new();
    let json = Some("application/json");
    for (path, file) in [
        ("", "services"),
        ("/Elevation", "elevation"),
        ("/Census/MapServer", "census"),
        (
            "/CommercialDamageAssessment/FeatureServer",
            "commercial-damage",
        ),
        ("/Elevation/GlacierBay/MapServer", "glacier-bay"),
    ] {
        f.insert(
            format!("{GIS}{path}?f=json"),
            json,
            fixture(&format!("arcgis-rest/{file}.json")),
        );
    }
    f
}

fn records_fetcher() -> MapFetcher {
    let f = MapFetcher::new();
    let geo = Some("application/geo+json");
    f.insert(
        format!("{RECORDS}/items?limit=100&f=json"),
        geo,
        fixture("ogc-records/page1.json"),
    );
    f.insert(
        format!("{RECORDS}/items?f=json&offset=2&limit=100"),
        geo,
        fixture("ogc-records/page2.json"),
    );
    f
}

fn title(ds: &Dataset) -> &str {
    ds.title.get("sk").map_or("", String::as_str)
}

#[test]
fn a_map_server_is_one_record_with_its_extent_copyright_and_a_geojson_link_per_layer() {
    let (org, cat) = (
        OrgId::from_uuid(Uuid::nil()),
        CatalogueId::from_uuid(Uuid::nil()),
    );
    let url = format!("{GIS}/Census/MapServer");
    let ds = parse_arcgis_service(
        org,
        cat,
        &gis_source(),
        &url,
        "Census/MapServer",
        &fixture("arcgis-rest/census.json"),
        "sk",
    )
    .expect("a service");
    assert_eq!(title(&ds), "Census");
    assert!(ds.description["sk"].starts_with("This map contains sample census data"));
    assert_eq!(ds.landing_page.as_deref(), Some(url.as_str()));
    assert_eq!(
        ds.harvest_source_item_id.as_deref(),
        Some("Census/MapServer")
    );
    assert_eq!(ds.licence_id, None, "a copyright line names no licence");
    assert_eq!(
        ds.extras["licence_text"],
        "US Bureau of the Census: http://www.census.gov"
    );
    let spatial: Value = serde_json::from_str(&ds.extras["spatial"]).unwrap();
    assert_eq!(spatial["type"], "Polygon");
    assert!((spatial["coordinates"][0][0][0].as_f64().unwrap() + 179.62).abs() < 0.01);
    assert_eq!(ds.distributions[0].access_url, url);
    assert_eq!(ds.distributions[0].format.as_deref(), Some("ArcGIS REST"));
    let layer = &ds.distributions[1];
    assert_eq!(layer.access_url, format!("{url}/0"));
    assert_eq!(
        layer.download_url.as_deref(),
        Some(format!("{url}/0/query?where=1%3D1&outFields=*&f=geojson").as_str())
    );
    assert_eq!(layer.media_type.as_deref(), Some("application/geo+json"));
    assert!(ds.harvest_hash.is_some());
}

#[test]
fn a_service_with_empty_fields_falls_back_to_its_name_and_an_unknown_reference_has_no_extent() {
    let (org, cat) = (
        OrgId::from_uuid(Uuid::nil()),
        CatalogueId::from_uuid(Uuid::nil()),
    );
    let ds = parse_arcgis_service(
        org,
        cat,
        &gis_source(),
        &format!("{GIS}/CommercialDamageAssessment/FeatureServer"),
        "CommercialDamageAssessment/FeatureServer",
        &fixture("arcgis-rest/commercial-damage.json"),
        "sk",
    )
    .expect("a service");
    assert_eq!(title(&ds), "CommercialDamageAssessment");
    assert_eq!(ds.description["sk"], "CommercialDamageAssessment");
    assert!(!ds.extras.contains_key("spatial"), "a state plane extent");
    assert!(!ds.extras.contains_key("licence_text"));
    assert_eq!(ds.distributions.len(), 2);
}

#[test]
fn a_service_answer_that_is_not_a_service_is_a_skip_reason() {
    let (org, cat) = (
        OrgId::from_uuid(Uuid::nil()),
        CatalogueId::from_uuid(Uuid::nil()),
    );
    for (body, reason) in [
        ("<html>login</html>", "the service answers no JSON"),
        (
            r#"{"error":{"code":499,"message":"Token Required"}}"#,
            "the service answers error 499",
        ),
        (
            r#"{"layers":"x"}"#,
            "the answer is not an ArcGIS service's JSON",
        ),
        ("[]", "the answer is not an ArcGIS service's JSON"),
    ] {
        let err = parse_arcgis_service(
            org,
            cat,
            &gis_source(),
            &format!("{GIS}/X/MapServer"),
            "X/MapServer",
            body,
            "sk",
        )
        .expect_err(body);
        assert_eq!(err, reason);
    }
}

#[test]
fn a_directory_lists_its_folders_and_services_and_refuses_names_that_leave_it() {
    let dir = parse_arcgis_directory(&fixture("arcgis-rest/services.json")).unwrap();
    assert_eq!(dir.folders, ["Elevation"]);
    assert_eq!(dir.services.len(), 4);
    let odd = parse_arcgis_directory(
        r#"{"folders":["../admin","a//b","ok"],"services":[{"name":"x?y","type":"MapServer"},
            {"name":"good","type":"MapServer"},{"name":"t"}]}"#,
    )
    .unwrap();
    assert_eq!(odd.folders, ["ok"]);
    assert_eq!(odd.services, [("good".to_owned(), "MapServer".to_owned())]);
    for bad in [r#"{"error":{"code":499}}"#, r#"{"layers":[]}"#, "nope"] {
        assert!(parse_arcgis_directory(bad).is_err(), "{bad}");
    }
    assert_eq!(
        arcgis_services_root(&format!("{GIS}/Elevation/?f=json")),
        GIS
    );
}

#[test]
fn extents_and_licences_read_as_the_catalogue_knows_them() {
    let merc = json!({"xmin": 1_900_000.0, "ymin": 6_100_000.0, "xmax": 1_950_000.0,
                      "ymax": 6_150_000.0, "spatialReference": {"wkid": 102100}});
    let [w, s, _, _] = arcgis_extent_wgs84(&merc).unwrap();
    assert!(
        (w - 17.068).abs() < 0.01 && (s - 47.959).abs() < 0.01,
        "{w} {s}"
    );
    let other = json!({"xmin": 0, "ymin": 0, "xmax": 1, "ymax": 1,
                       "spatialReference": {"wkid": 5514}});
    assert_eq!(arcgis_extent_wgs84(&other), None);
    assert_eq!(open_licence("CC-BY-4.0").map(|l| l.0), Some("cc-by"));
    assert_eq!(
        open_licence("http://creativecommons.org/publicdomain/zero/1.0/").map(|l| l.0),
        Some("cc-zero")
    );
    assert_eq!(open_licence("© Esri"), None);
}

#[test]
fn a_records_page_keeps_its_records_and_skips_a_malformed_one_with_its_reason() {
    let (org, cat) = (
        OrgId::from_uuid(Uuid::nil()),
        CatalogueId::from_uuid(Uuid::nil()),
    );
    let url = format!("{RECORDS}/items?limit=100&f=json");
    let page = parse_ogc_records_page(
        org,
        cat,
        &records_source(),
        &url,
        &fixture("ogc-records/page1.json"),
        "sk",
    )
    .unwrap();
    assert_eq!((page.read, page.matched, page.kept.len()), (2, Some(4), 2));
    assert_eq!(
        page.next.as_deref(),
        Some(format!("{RECORDS}/items?f=json&offset=2&limit=100").as_str())
    );
    let (id, ds) = &page.kept[0];
    assert_eq!(id, "35149dfb-31d3-431c-a8bc-12a4034dac48");
    assert_eq!(title(ds), "Kaartboeck 1635");
    assert_eq!(
        ds.uri.as_deref(),
        Some(format!("{RECORDS}/items/{id}").as_str())
    );
    assert!(ds.tags.contains(&"MONUMENTEN".to_owned()), "{:?}", ds.tags);
    assert_eq!(ds.modified, "2025-06-16T12:39:57Z".parse().unwrap());
    assert!(ds.extras["spatial"].contains("52.6333984375"));
    assert_eq!(ds.distributions.len(), 4);
    assert_eq!(ds.distributions[0].format.as_deref(), Some("WMS"));
    assert_eq!(
        ds.distributions[2].format, None,
        "type `download` names no format"
    );
    let p2 = parse_ogc_records_page(
        org,
        cat,
        &records_source(),
        &url,
        &fixture("ogc-records/page2.json"),
        "sk",
    )
    .unwrap();
    assert_eq!((p2.read, p2.kept.len(), p2.next), (2, 0, None));
    let reasons: Vec<_> = p2.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(reasons, ["the record has no title", "the record has no id"]);
    for bad in ["{}", "nope", r#"{"features":{}}"#] {
        assert!(
            parse_ogc_records_page(org, cat, &records_source(), &url, bad, "sk").is_err(),
            "{bad}"
        );
    }
}

#[tokio::test]
async fn an_arcgis_directory_harvests_its_map_and_feature_servers_and_reruns_changed_only() {
    let w = world(&[]).await;
    let f = gis_fetcher();
    let e = engine(&f);
    let run = e
        .run_source(&w.ctx, &w.store, &gis_source())
        .await
        .expect("run");
    assert_eq!(
        (run.state, run.created_count),
        (HarvestRunState::Succeeded, 3)
    );
    let reasons: Vec<_> = run.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(reasons.len(), 3, "{reasons:?}");
    assert!(
        reasons.contains(&"listed as its FeatureServer"),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"type GPServer is not a dataset"),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"type ImageServer is not a dataset"),
        "{reasons:?}"
    );
    assert_eq!(active_names(&w).await.len(), 3);
    let again = e
        .run_source(&w.ctx, &w.store, &gis_source())
        .await
        .expect("run 2");
    assert_eq!((again.created_count, again.unchanged_count), (0, 3));
}

#[tokio::test]
async fn an_arcgis_url_naming_one_service_reads_that_service_alone() {
    let w = world(&[]).await;
    let mut src = gis_source();
    src.url = format!("{GIS}/Elevation/GlacierBay/MapServer");
    let run = engine(&gis_fetcher())
        .run_source(&w.ctx, &w.store, &src)
        .await
        .expect("run");
    assert_eq!(run.created_count, 1);
    let ds = w.store.list_datasets(&w.ctx).await.unwrap();
    assert_eq!(
        ds[0].harvest_source_item_id.as_deref(),
        Some("Elevation/GlacierBay/MapServer")
    );
    assert_eq!(title(&ds[0]), "Glacier Bay Elevation");
}

#[tokio::test]
async fn an_ogc_records_collection_reads_every_page_and_reruns_changed_only() {
    let w = world(&[]).await;
    let f = records_fetcher();
    let e = engine(&f);
    let run = e
        .run_source(&w.ctx, &w.store, &records_source())
        .await
        .expect("run");
    assert_eq!((run.created_count, run.skipped.len()), (2, 2));
    let again = e
        .run_source(&w.ctx, &w.store, &records_source())
        .await
        .expect("run 2");
    assert_eq!((again.created_count, again.unchanged_count), (0, 2));
}

#[tokio::test]
async fn an_ogc_records_read_short_of_number_matched_changes_nothing() {
    let w = world(&[]).await;
    let f = records_fetcher();
    let short =
        fixture("ogc-records/page2.json").replace("\"numberMatched\": 4", "\"numberMatched\": 9");
    let first =
        fixture("ogc-records/page1.json").replace("\"numberMatched\": 4", "\"numberMatched\": 9");
    f.insert(
        format!("{RECORDS}/items?limit=100&f=json"),
        Some("application/geo+json"),
        first,
    );
    f.insert(
        format!("{RECORDS}/items?f=json&offset=2&limit=100"),
        Some("application/geo+json"),
        short,
    );
    let err = engine(&f)
        .run_source(&w.ctx, &w.store, &records_source())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("got 4 of 9 records"), "{err}");
    assert!(active_names(&w).await.is_empty());
}

#[tokio::test]
async fn a_run_past_its_budget_changes_nothing_for_either_type() {
    for (mut src, f) in [
        (gis_source(), gis_fetcher()),
        (records_source(), records_fetcher()),
    ] {
        let w = world(&[]).await;
        src.budget = Some(HarvestBudget {
            records: 1,
            bytes: 50_000_000,
        });
        let err = engine(&f)
            .run_source(&w.ctx, &w.store, &src)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("past its budget of 1"),
            "{}: {err}",
            src.name
        );
        assert!(active_names(&w).await.is_empty(), "{}", src.name);
    }
}

#[tokio::test]
async fn a_redirect_off_the_allowed_host_is_refused_and_nothing_is_written() {
    for (src, url) in [
        (gis_source(), format!("{GIS}/Census/MapServer?f=json")),
        (
            records_source(),
            format!("{RECORDS}/items?f=json&offset=2&limit=100"),
        ),
    ] {
        let w = world(&[]).await;
        let f = if src.name == "gis" {
            gis_fetcher()
        } else {
            records_fetcher()
        };
        f.insert_redirect(url, 302, "https://attacker.example/steal");
        let err = engine(&f)
            .run_source(&w.ctx, &w.store, &src)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("attacker.example"),
            "{}: {err}",
            src.name
        );
        assert!(active_names(&w).await.is_empty(), "{}", src.name);
    }
}

#[tokio::test]
async fn a_source_whose_name_resolves_to_a_private_address_is_refused() {
    for (src, f) in [
        (gis_source(), gis_fetcher()),
        (records_source(), records_fetcher()),
    ] {
        let w = world(&[]).await;
        let e = HarvestEngine::new(Arc::new(f.clone()))
            .with_custom_dns(|_, port| Ok(vec![SocketAddr::from(([10, 0, 0, 7], port))]));
        let err = e.run_source(&w.ctx, &w.store, &src).await.unwrap_err();
        let private = std::net::Ipv4Addr::new(10, 0, 0, 7).to_string();
        assert!(err.to_string().contains(&private), "{}: {err}", src.name);
        assert!(f.pinned_seen().is_empty(), "{}", src.name);
        assert!(active_names(&w).await.is_empty(), "{}", src.name);
    }
}

#[tokio::test]
async fn the_source_test_reads_the_first_answer_of_each_type_and_names_a_wrong_one() {
    let w = world(&[]).await;
    let f = gis_fetcher();
    f.insert(
        format!("{RECORDS}/items?limit=1&f=json"),
        Some("application/geo+json"),
        fixture("ogc-records/page1.json"),
    );
    let e = engine(&f);
    for src in [gis_source(), records_source()] {
        let got = e.test_source(&w.ctx, &w.store, &src).await.unwrap();
        assert_eq!(got, SourceTest::Ok, "{}", src.name);
    }
    let mut wrong = gis_source();
    wrong.kind = HarvestSourceKind::OgcRecords;
    f.insert(
        format!("{GIS}/items?limit=1&f=json"),
        Some("application/json"),
        fixture("arcgis-rest/services.json"),
    );
    let got = e.test_source(&w.ctx, &w.store, &wrong).await.unwrap();
    assert_eq!(got, SourceTest::NotTheExpectedFormat);
}
