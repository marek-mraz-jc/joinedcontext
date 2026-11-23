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

#[test]
fn a_record_reads_a_numeric_id_its_licence_link_bbox_html_page_and_link_formats() {
    let (org, cat) = (
        OrgId::from_uuid(Uuid::nil()),
        CatalogueId::from_uuid(Uuid::nil()),
    );
    let url = format!("{RECORDS}/items?limit=100&f=json");
    let link = |t: &str, href: &str| json!({"rel": "item", "type": t, "href": href, "title": ""});
    let page = json!({"features": [{
        "id": 7,
        "bbox": [17.0, 48.1, 17.2, 48.2],
        "properties": {"title": "Stromy &amp; parky", "description": "", "created": "2026-01-02T00:00:00Z"},
        "links": [
            {"rel": "license", "href": "https://creativecommons.org/licenses/by/4.0/"},
            {"rel": "alternate", "type": "text/html", "href": "../html/7"},
            link("text/csv", "a.csv"), link("application/zip", "a.zip"),
            link("application/pdf", "a.pdf"), link("text/xml", "a.xml"),
            link("application/x-shapefile", "a.shp"), link("OGC:WFS", "wfs"),
            link("application/vnd.custom", "a.bin"),
        ]
    }]})
    .to_string();
    let mut src = records_source();
    src.spelling_fixes
        .insert("parky".into(), "parky mesta".into());
    src.target_publisher_iri = Some("https://data.gov.cz/zdroj/ovm/1".into());
    let got = parse_ogc_records_page(org, cat, &src, &url, &page, "sk").expect("page");
    let (id, ds) = &got.kept[0];
    assert_eq!(id, "7");
    let publisher = ds.publisher.as_ref().expect("the publisher IRI");
    assert_eq!(
        publisher.uri.as_deref(),
        Some("https://data.gov.cz/zdroj/ovm/1")
    );
    assert_eq!(publisher.name["sk"], "city");
    assert_eq!(
        title(ds),
        "Stromy & parky mesta",
        "entities decoded, spelling fixed"
    );
    assert_eq!(
        ds.description["sk"], "Stromy & parky mesta",
        "no description: the title"
    );
    assert_eq!(
        ds.licence_id.as_deref(),
        Some("cc-by"),
        "{:?}",
        ds.licence_id
    );
    assert!(ds.distributions.iter().all(|d| d.licence == ds.licence_url));
    assert!(
        ds.extras["spatial"].contains("48.2"),
        "the bbox array: {:?}",
        ds.extras
    );
    assert_eq!(
        ds.landing_page.as_deref(),
        Some("https://demo.pygeoapi.io/master/collections/html/7")
    );
    let formats: Vec<Option<&str>> = ds
        .distributions
        .iter()
        .map(|d| d.format.as_deref())
        .collect();
    assert_eq!(
        formats,
        [
            Some("CSV"),
            Some("ZIP"),
            Some("PDF"),
            Some("XML"),
            Some("SHP"),
            Some("WFS"),
            Some("application/vnd.custom")
        ]
    );
    assert!(
        ds.distributions
            .iter()
            .all(|d| d.title["sk"] == "Stromy & parky mesta"),
        "an empty link title takes the record's"
    );
    // A page address that does not parse is refused.
    assert!(parse_ogc_records_page(org, cat, &src, "not a url", &page, "sk").is_err());
}

#[test]
fn a_service_reads_its_initial_extent_and_skips_a_layer_without_an_id() {
    let (org, cat) = (
        OrgId::from_uuid(Uuid::nil()),
        CatalogueId::from_uuid(Uuid::nil()),
    );
    let body = json!({
        "serviceDescription": "Stromy",
        "initialExtent": {"xmin": 17.0, "ymin": 48.1, "xmax": 17.2, "ymax": 48.2, "spatialReference": {"wkid": 4326}},
        "layers": [{"name": "no id"}, {"id": 3, "name": "Stromy"}]
    })
    .to_string();
    let ds = parse_arcgis_service(
        org,
        cat,
        &gis_source(),
        &format!("{GIS}/Trees/MapServer"),
        "Trees/MapServer",
        &body,
        "sk",
    )
    .expect("a service");
    assert!(ds.extras["spatial"].contains("48.2"), "{:?}", ds.extras);
    let urls: Vec<&str> = ds
        .distributions
        .iter()
        .map(|d| d.access_url.as_str())
        .collect();
    assert_eq!(
        urls,
        [
            format!("{GIS}/Trees/MapServer"),
            format!("{GIS}/Trees/MapServer/3")
        ],
        "the service, then only the layer with an id"
    );
    assert_eq!(
        arcgis_services_root("https://gis.example/arcgis/x?f=json"),
        "https://gis.example/arcgis/x"
    );
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
async fn a_service_the_source_skips_by_id_is_listed_as_skipped_with_its_title() {
    let w = world(&[]).await;
    let mut src = gis_source();
    src.skip_items
        .insert("Elevation/GlacierBay/MapServer".into(), "not ours".into());
    let run = engine(&gis_fetcher())
        .run_source(&w.ctx, &w.store, &src)
        .await
        .expect("run");
    let skipped: Vec<(&str, &str)> = run
        .skipped
        .iter()
        .filter(|s| s.item_id == "Elevation/GlacierBay/MapServer")
        .map(|s| (s.title.as_str(), s.reason.as_str()))
        .collect();
    assert_eq!(skipped, [("Glacier Bay Elevation", "not ours")]);
    assert!(run.created_count > 0, "the other services are kept");
}

/// A records page whose `next` is `next` and whose features are `ids`.
fn records_page(ids: &[&str], matched: Option<usize>, next: Option<&str>) -> String {
    let features: Vec<Value> = ids
        .iter()
        .map(|id| json!({"id": id, "properties": {"title": format!("Record {id}")}, "links": []}))
        .collect();
    let mut links = vec![];
    if let Some(n) = next {
        links.push(json!({"rel": "next", "href": n}));
    }
    let mut page = json!({"features": features, "links": links});
    if let Some(m) = matched {
        page["numberMatched"] = json!(m);
    }
    page.to_string()
}

#[tokio::test]
async fn a_records_read_that_loops_pages_forever_or_repeats_an_id_changes_nothing() {
    let first = format!("{RECORDS}/items?limit=100&f=json");
    let geo = Some("application/geo+json");
    let cases: Vec<(&str, MapFetcher)> = vec![
        ("the pages loop back", {
            let f = MapFetcher::new();
            f.insert(&first, geo, records_page(&["a"], None, Some(&first)));
            f
        }),
        ("still pages after", {
            let f = MapFetcher::new();
            f.insert(
                &first,
                geo,
                records_page(&["p0"], None, Some(&format!("{RECORDS}/p/1"))),
            );
            for i in 1..=60 {
                let id = format!("p{i}");
                f.insert(
                    format!("{RECORDS}/p/{i}"),
                    geo,
                    records_page(
                        &[id.as_str()],
                        None,
                        Some(&format!("{RECORDS}/p/{}", i + 1)),
                    ),
                );
            }
            f
        }),
        ("1 of them distinct", {
            let f = MapFetcher::new();
            f.insert(&first, geo, records_page(&["a", "a"], Some(2), None));
            f
        }),
    ];
    for (want, f) in cases {
        let w = world(&[]).await;
        let res = engine(&f)
            .run_source(&w.ctx, &w.store, &records_source())
            .await;
        let runs = w.store.list_harvest_runs(&w.ctx, None).await.unwrap();
        let said = res
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default()
            + runs[0].error.as_deref().unwrap_or("");
        assert!(said.contains(want), "{want}: {said}");
        assert!(active_names(&w).await.is_empty(), "{want}");
    }
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
async fn the_source_test_reads_a_service_and_a_sparql_answer_and_needs_its_catalogue() {
    let w = world(&[]).await;
    let f = MapFetcher::new();
    let service = format!("{GIS}/Trees/MapServer");
    f.insert(
        format!("{service}?f=json"),
        Some("application/json"),
        json!({"layers": []}).to_string(),
    );
    let nkod_url = "https://data.gov.cz/sparql?query=SELECT%201";
    f.insert(
        nkod_url,
        Some("application/sparql-results+json"),
        json!({"results": {"bindings": []}}).to_string(),
    );
    let e = engine(&f);
    let mut gis = gis_source();
    gis.url = service.clone();
    let nkod = source("nkod-sparql", "nkod", nkod_url, &["data.gov.cz"]);
    for src in [&gis, &nkod] {
        let got = e.test_source(&w.ctx, &w.store, src).await.unwrap();
        assert_eq!(got, SourceTest::Ok, "{}", src.name);
    }
    // An error answer is no service; bindings that are no list are no SPARQL answer.
    f.insert(
        format!("{service}?f=json"),
        Some("application/json"),
        json!({"error": {"code": 499}}).to_string(),
    );
    f.insert(
        nkod_url,
        Some("application/json"),
        json!({"results": {"bindings": {}}}).to_string(),
    );
    for src in [&gis, &nkod] {
        let got = e.test_source(&w.ctx, &w.store, src).await.unwrap();
        assert_eq!(got, SourceTest::NotTheExpectedFormat, "{}", src.name);
    }
    // Another catalogue's context finds no catalogue.
    let other = dataroom_core::RequestCtx::new(
        OrgId::from_uuid(Uuid::now_v7()),
        CatalogueId::from_uuid(Uuid::now_v7()),
        w.ctx.subject.clone(),
    );
    let err = e.test_source(&other, &w.store, &gis).await.unwrap_err();
    assert!(
        matches!(err, dataroom_core::DataroomError::NotFound(_)),
        "{err}"
    );
}

#[tokio::test]
async fn the_source_test_reads_ckan_and_hub_first_pages_and_refuses_an_nkod_source_without_its_iri()
{
    let w = world(&[]).await;
    let f = MapFetcher::new();
    let ckan = "https://ckan.remote.example";
    f.insert(
        format!("{ckan}/api/3/action/package_search?rows=1&start=0"),
        Some("application/json"),
        ckan_page(0, vec![]),
    );
    f.insert(
        format!("{ckan}/api/3/action/package_search?q=air&rows=1&start=0"),
        Some("application/json"),
        ckan_page(0, vec![]),
    );
    f.insert(
        format!("{ckan}/api/3/action/package_search?rows=1&start=0&x=1"),
        Some("application/json"),
        "not json",
    );
    let hub = "https://hub.city.example";
    f.insert(
        format!("{hub}/api/search/v1/collections/dataset/items?limit=1"),
        Some("application/json"),
        json!({"features": []}).to_string(),
    );
    let e = engine(&f);
    let hosts = ["ckan.remote.example", "hub.city.example", "data.gov.cz"];
    let cases = [
        (source("ckan-action", "c1", ckan, &hosts), SourceTest::Ok),
        (
            source(
                "ckan-action",
                "c2",
                &format!("{ckan}/api/3/action/package_search?q=air"),
                &hosts,
            ),
            SourceTest::Ok,
        ),
        (source("arc-gis-hub", "h", hub, &hosts), SourceTest::Ok),
        (
            source("nkod-sparql", "n", "https://data.gov.cz/sparql", &hosts),
            SourceTest::RefusedHost,
        ),
    ];
    for (src, want) in cases {
        let got = e.test_source(&w.ctx, &w.store, &src).await.unwrap();
        assert_eq!(got, want, "{}", src.name);
    }
}

#[tokio::test]
async fn a_preview_whose_listing_fails_is_partial_with_no_records() {
    let w = world(&[]).await;
    // The test's one-record page answers; the listing's first page does not.
    let f = MapFetcher::new();
    f.insert(
        format!("{RECORDS}/items?limit=1&f=json"),
        Some("application/geo+json"),
        fixture("ogc-records/page1.json"),
    );
    let p = engine(&f)
        .preview_source(&w.ctx, &w.store, &records_source())
        .await
        .expect("preview");
    assert_eq!(p.outcome, SourceTest::Ok);
    assert!(p.partial);
    assert!(p.records.is_empty());
}

#[tokio::test]
async fn a_preview_lists_within_the_sources_budget_and_needs_its_catalogue() {
    let w = world(&[]).await;
    let f = records_fetcher();
    f.insert(
        format!("{RECORDS}/items?limit=1&f=json"),
        Some("application/geo+json"),
        fixture("ogc-records/page1.json"),
    );
    let e = engine(&f);
    let mut src = records_source();
    src.budget = Some(HarvestBudget {
        records: 1,
        bytes: 10_000_000,
    });
    let p = e
        .preview_source(&w.ctx, &w.store, &src)
        .await
        .expect("preview");
    assert_eq!(p.outcome, SourceTest::Ok);
    assert!(
        !p.records.is_empty(),
        "the page listed within its byte budget"
    );
    let other = dataroom_core::RequestCtx::new(
        OrgId::from_uuid(Uuid::now_v7()),
        CatalogueId::from_uuid(Uuid::now_v7()),
        w.ctx.subject.clone(),
    );
    let err = e.preview_source(&other, &w.store, &src).await.unwrap_err();
    assert!(
        matches!(err, dataroom_core::DataroomError::NotFound(_)),
        "{err}"
    );
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
