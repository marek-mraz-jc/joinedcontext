//! The source parsers on their own: what each keeps, what it skips and why, and the record it
//! builds (ids, names, licences, notes, tags, distribution URLs).

mod common;

use common::source;
use dataroom_core::{CatalogueId, HarvestSource, OrgId};
use dataroom_harvest::sources::{
    ascii_slug, compute_harvest_hash, parse_arcgis_hub_items, parse_ckan_search_page,
    parse_dcat_page, parse_nkod_sparql_bindings, plain_text_from_html,
};
use serde_json::{json, Value};
use uuid::Uuid;

fn ids() -> (OrgId, CatalogueId) {
    (
        OrgId::from_uuid(Uuid::nil()),
        CatalogueId::from_uuid(Uuid::nil()),
    )
}

fn with(mut src: HarvestSource, f: impl FnOnce(&mut HarvestSource)) -> HarvestSource {
    f(&mut src);
    src
}

fn reasons(skipped: &[dataroom_core::HarvestSkippedItem]) -> Vec<(String, String)> {
    skipped
        .iter()
        .map(|s| (s.item_id.clone(), s.reason.clone()))
        .collect()
}

#[test]
fn plain_text_keeps_paragraphs_and_decodes_references() {
    assert_eq!(
        plain_text_from_html("<p>Prvý&nbsp;odsek</p><div>Druhý <b>tučný</b><br>riadok</div>"),
        "Prvý odsek\n\nDruhý tučný\n\nriadok"
    );
    assert_eq!(
        plain_text_from_html("Ceny &lt;5 &amp; &#62;3 &#x20AC; &quot;a&quot; &apos;b&apos;"),
        "Ceny <5 & >3 € \"a\" 'b'"
    );
    // A `<` that opens no tag is text, an unknown or broken reference stays as written.
    assert_eq!(plain_text_from_html("a < b, 3<4"), "a < b, 3<4");
    assert_eq!(
        plain_text_from_html("&copy; &#xZZ; &#99999999; & ;&verylongname;"),
        "&copy; &#xZZ; &#99999999; & ;&verylongname;"
    );
    // Comments vanish whole, even with a `>` inside; an unclosed tag swallows the rest.
    assert_eq!(plain_text_from_html("x<!-- a > b -->y"), "xy");
    assert_eq!(plain_text_from_html("x<!-- never closed"), "x");
    assert_eq!(plain_text_from_html("x<a href=\"y"), "x");
    // Only a start tag breaks a line, as in HTMLParser.handle_starttag.
    assert_eq!(
        plain_text_from_html("<H2 class=t>Nadpis</H2><p>text<?php ?>"),
        "Nadpis\n\ntext"
    );
    assert_eq!(plain_text_from_html("<b>Nadpis</b></p>text"), "Nadpistext");
    assert_eq!(plain_text_from_html(""), "");
    assert_eq!(
        plain_text_from_html(&"á".repeat(20_000)).chars().count(),
        10_000
    );
}

#[test]
fn slugs_and_hashes() {
    assert_eq!(
        ascii_slug("Kvalita ovzdušia — Žilina 2026!"),
        "kvalita-ovzdusia-zilina-2026"
    );
    assert_eq!(ascii_slug("--Ábc__d--"), "abc-d");
    assert_eq!(ascii_slug("日本"), "");

    let (org, cat) = ids();
    let src = source("ckan-action", "r", "https://ckan.remote.example", &[]);
    let page = json!({"success": true, "result": {"count": 1, "results": [
        common::ckan_package("p-1", "air", "Ovzdušie")
    ]}})
    .to_string();
    let (mut rows, _) = parse_ckan_search_page(org, cat, &src, &page, "sk").unwrap();
    let mut ds = rows.remove(0).1;
    let h = compute_harvest_hash(&ds);
    assert_eq!(h.len(), 64);
    assert_eq!(ds.harvest_hash.as_deref(), Some(h.as_str()));
    ds.extras.insert("noise".into(), "not hashed".into());
    assert_eq!(
        compute_harvest_hash(&ds),
        h,
        "only catalogue content is hashed"
    );
    ds.distributions[0].access_url.push_str("?v=2");
    assert_ne!(compute_harvest_hash(&ds), h);
}

fn hub(id: &str, title: &str, kind: &str, licence: &str, modified: i64) -> Value {
    json!({"properties": {"id": id, "title": title, "type": kind, "license": licence,
                          "modified": modified, "url": format!("https://services.city.example/{id}")}})
}

#[test]
fn hub_items_are_kept_or_skipped_with_a_reason() {
    let (org, cat) = ids();
    let src = with(
        source(
            "arc-gis-hub",
            "hub",
            "https://hub.city.example/",
            &["services.city.example"],
        ),
        |s| {
            s.skip_items
                .insert("skipme".into(), "the city asked".into());
            s.title_overrides
                .insert("over".into(), "Prepísaný  názov".into());
            s.spelling_fixes.insert("Kvalia".into(), "Kvalita".into());
        },
    );
    let mut foreign = hub("evil", "Cudzia služba", "Feature Service", "CC0-1.0", 1);
    foreign["properties"]["url"] = json!("https://evil.example/FeatureServer");
    let features = vec![
        json!({"properties": {"title": "no id"}}),
        hub("skipme", "Skip", "Shapefile", "CC-BY-4.0", 1),
        hub(
            "app",
            "Aplikácia",
            "Web Mapping Application",
            "CC-BY-4.0",
            1,
        ),
        hub("closed", "Uzavretá", "Shapefile", "proprietary", 1),
        foreign,
        hub("old", "Kvalia ovzdušia", "Shapefile", "CC-BY-4.0", 100),
        hub(
            "new",
            "Kvalita ovzdušia",
            "Feature Service",
            "CC-BY-SA-4.0",
            200,
        ),
        hub("older", "Kvalita ovzdušia", "Shapefile", "CC-BY-4.0", 50),
        hub("over", "ignored", "Shapefile", "CC0-1.0", 1),
    ];
    let (kept, skipped) = parse_arcgis_hub_items(org, cat, &src, &[], &features, "sk");
    assert_eq!(
        reasons(&skipped),
        [
            ("skipme".into(), "the city asked".into()),
            (
                "app".into(),
                "type Web Mapping Application is not a dataset".into()
            ),
            ("closed".into(), "no open licence (proprietary)".into()),
            (
                "evil".into(),
                "its service is not on an allowed host".into()
            ),
            ("old".into(), "duplicate of new".into()),
            ("older".into(), "duplicate of new".into()),
        ]
    );
    let names: Vec<_> = kept
        .iter()
        .map(|(id, d)| (id.as_str(), d.name.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("new", "city-kvalita-ovzdusia-new"),
            ("over", "city-prepisany-nazov-over")
        ]
    );
    let (_, ds) = &kept[0];
    assert_eq!(ds.licence_id.as_deref(), Some("cc-by-sa"));
    assert_eq!(
        ds.id,
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            b"https://hub.city.example/datasets/new"
        )
        .to_string()
    );
    assert_eq!(
        ds.distributions[0].access_url,
        "https://services.city.example/new"
    );
    assert_eq!(kept[1].1.title["sk"], "Prepísaný názov");
    assert_eq!(kept[1].1.licence_id.as_deref(), Some("cc-zero"));
}

#[test]
fn hub_notes_and_tags_follow_the_legacy_harvester() {
    let (org, cat) = ids();
    let src = with(
        source("arc-gis-hub", "hub", "https://hub.city.example", &[]),
        |s| s.target_publisher_iri = Some("https://data.example/org/city".into()),
    );
    let item = |extra: Value| {
        let mut v = json!({"id": "a1", "title": "Doprava", "type": "Shapefile",
                           "license": "CC-BY-4.0"});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    };
    let parse = |v: Value| {
        parse_arcgis_hub_items(org, cat, &src, &[], &[v], "sk")
            .0
            .remove(0)
            .1
    };

    let ds = parse(item(json!({
        "description": "<p></p>", "snippet": "Krátky &amp; jasný",
        "tags": [" doprava ", "x", "<script>", "a/b", "MHD 2026", "x".repeat(101)],
        "categories": ["/Categories/Mobilita", "/Categories/Životné prostredie", "/c/!!"]
    })));
    assert_eq!(
        ds.description["sk"], "Krátky & jasný",
        "empty markup falls to the snippet"
    );
    assert_eq!(
        ds.tags,
        ["MHD 2026", "Mobilita", "doprava", "Životné prostredie"]
    );
    assert_eq!(ds.keywords["sk"], ds.tags);
    assert_eq!(
        ds.publisher.as_ref().and_then(|p| p.uri.as_deref()),
        Some("https://data.example/org/city")
    );
    // No Feature Service URL: the distribution points at the Hub page.
    assert_eq!(
        ds.distributions[0].access_url,
        "https://hub.city.example/datasets/a1"
    );

    let ds = parse(item(json!({"description": "<b>Popis</b>"})));
    assert_eq!(ds.description["sk"], "Popis");
    let ds = parse(item(json!({})));
    assert_eq!(ds.description["sk"], "Doprava", "no text at all: the title");
}

fn binding(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), json!({"type": "literal", "value": v})))
            .collect(),
    )
}

#[test]
fn nkod_folds_rows_and_keeps_only_open_licences_on_allowed_hosts() {
    let (org, cat) = ids();
    let src = with(
        source(
            "nkod-sparql",
            "nkod",
            "https://data.gov.cz/sparql",
            &["www.brno.cz", "data.gov.cz"],
        ),
        |s| {
            s.name_prefix = "cz-".into();
            s.rewrite_hosts
                .insert("brno.cz".into(), "www.brno.cz".into());
            s.skip_items
                .insert("Vynechaná".into(), "duplicate of a local dataset".into());
        },
    );
    let eu = "http://publications.europa.eu/resource/authority/licence/";
    let rows = json!({"results": {"bindings": [
        binding(&[("title", "No IRI")]),
        binding(&[("ds", "https://d/x")]),
        binding(&[("ds", "https://d/skip"), ("title", "Vynechaná")]),
        binding(&[("ds", "https://d/nodist"), ("title", "Bez distribúcie")]),
        // One dataset over three rows: keywords and licences fold, the stricter licence wins.
        binding(&[("ds", "https://d/air"), ("title", "Ovzdušie"), ("keyword", "ovzdušie"),
                  ("landing", "https://brno.cz/air"),
                  ("dist", "https://d/air/1"), ("url", "https://brno.cz/data/ovzdušie.csv?q=á"),
                  ("format", "http://publications.europa.eu/resource/authority/file-type/CSV"),
                  ("work", &format!("{eu}CC_BY_4_0"))]),
        binding(&[("ds", "https://d/air"), ("title", "Ovzdušie"), ("keyword", ""),
                  ("dist", "https://d/air/2"), ("url", "https://data.gov.cz/air.json"),
                  ("distTitle", "JSON"), ("database", &format!("{eu}CC_BYSA_4_0"))]),
        binding(&[("ds", "https://d/air"), ("title", "Ovzdušie"), ("keyword", "kvalita")]),
        binding(&[("ds", "https://d/closed"), ("title", "Uzavretá"),
                  ("dist", "https://d/closed/1"), ("url", "https://www.brno.cz/c.csv"),
                  ("work", "https://example.org/proprietary")]),
        binding(&[("ds", "https://d/nolic"), ("title", "Bez licencie"),
                  ("dist", "https://d/nolic/1"), ("url", "https://www.brno.cz/n.csv")]),
        binding(&[("ds", "https://d/far"), ("title", "Cudzí server"), ("description", "Popis"),
                  ("dist", "https://d/far/1"), ("url", "https://evil.example/brno.cz/f.csv"),
                  ("special", "https://creativecommons.org/publicdomain/zero/1.0/")]),
    ]}});
    let (kept, skipped) = parse_nkod_sparql_bindings(org, cat, &src, &[], &rows, "cs");
    assert_eq!(
        reasons(&skipped),
        [
            (
                "https://d/closed".into(),
                "no open licence in legislation:termsOfUse".into()
            ),
            (
                "https://d/far".into(),
                "all distribution URLs refused by host allowlist".into()
            ),
            (
                "https://d/nodist".into(),
                "no distribution with a download URL".into()
            ),
            (
                "https://d/nolic".into(),
                "no open licence in legislation:termsOfUse".into()
            ),
            (
                "https://d/skip".into(),
                "duplicate of a local dataset".into()
            ),
        ]
    );
    assert_eq!(kept.len(), 1);
    let (item, ds) = &kept[0];
    assert_eq!(item, "https://d/air");
    assert_eq!(ds.name, "cz-ovzdusie");
    assert_eq!(ds.licence_id.as_deref(), Some("cc-by-sa"));
    assert_eq!(ds.tags, ["kvalita", "ovzdušie"]);
    assert_eq!(ds.description["cs"], "Zdroj: https://d/air");
    assert_eq!(ds.landing_page.as_deref(), Some("https://brno.cz/air"));
    let urls: Vec<_> = ds
        .distributions
        .iter()
        .map(|d| d.access_url.as_str())
        .collect();
    assert_eq!(
        urls,
        [
            // The exact host moves; the path is percent-encoded as a client sends it.
            "https://www.brno.cz/data/ovzdu%C5%A1ie.csv?q=%C3%A1",
            "https://data.gov.cz/air.json",
        ]
    );
    let formats: Vec<_> = ds
        .distributions
        .iter()
        .map(|d| d.format.as_deref())
        .collect();
    assert_eq!(formats, [Some("CSV"), Some("JSON")]);
    assert_eq!(ds.distributions[1].title["cs"], "JSON");
    assert_eq!(ds.distributions[0].title["cs"], "Ovzdušie");
}

#[test]
fn nkod_moves_only_the_exact_host() {
    let (org, cat) = ids();
    let src = with(
        source(
            "nkod-sparql",
            "nkod",
            "https://data.gov.cz/sparql",
            &["www.brno.cz", "www.www.brno.cz"],
        ),
        |s| {
            s.rewrite_hosts
                .insert("brno.cz".into(), "www.brno.cz".into());
        },
    );
    let rows = json!({"results": {"bindings": [
        binding(&[("ds", "https://d/a"), ("title", "A"), ("dist", "https://d/a/1"),
                  ("url", " https://www.brno.cz/brno.cz/a.csv?from=brno.cz "), ("work", "CC0")]),
    ]}});
    let (kept, skipped) = parse_nkod_sparql_bindings(org, cat, &src, &[], &rows, "cs");
    assert!(skipped.is_empty(), "{skipped:?}");
    assert_eq!(
        kept[0].1.distributions[0].access_url,
        "https://www.brno.cz/brno.cz/a.csv?from=brno.cz"
    );
}

#[test]
fn dcat_pages_are_read_in_any_rdf_syntax() {
    let (org, cat) = ids();
    let src = with(
        source("dcat-ap", "feed", "https://feeds.example.org/c", &[]),
        |s| {
            s.name_prefix = "feed-".into();
        },
    );
    let ttl = common::dcat_turtle(
        &[("https://feeds.example.org/ds/air", "Ovzdušie")],
        Some("https://feeds.example.org/c?page=2"),
    );
    for ct in [Some("text/turtle"), None] {
        let (rows, next) = parse_dcat_page(org, cat, &src, &ttl, ct, "sk").unwrap();
        assert_eq!(next.as_deref(), Some("https://feeds.example.org/c?page=2"));
        assert_eq!(rows.len(), 1);
        let (item, ds) = &rows[0];
        assert_eq!(item, "https://feeds.example.org/ds/air");
        assert!(ds.name.starts_with("feed-"), "{}", ds.name);
        assert_eq!(ds.harvest_source_id.as_deref(), Some("feed"));
        assert_eq!(
            ds.id,
            Uuid::new_v5(&Uuid::NAMESPACE_URL, item.as_bytes()).to_string()
        );
    }

    let jsonld = json!({
        "@context": {"dcat": "http://www.w3.org/ns/dcat#", "dct": "http://purl.org/dc/terms/",
                     "hydra": "http://www.w3.org/ns/hydra/core#"},
        "@graph": [
            {"@id": "https://feeds.example.org/ds/noise", "@type": "dcat:Dataset",
             "dct:title": {"@value": "Hluk", "@language": "sk"}},
            {"@id": "https://feeds.example.org/c", "hydra:nextPage": {"@id": "https://feeds.example.org/c?p=3"}}
        ]
    })
    .to_string();
    let (rows, next) = parse_dcat_page(org, cat, &src, &jsonld, None, "sk").unwrap();
    assert_eq!(next.as_deref(), Some("https://feeds.example.org/c?p=3"));
    assert_eq!(rows[0].0, "https://feeds.example.org/ds/noise");

    let rdfxml = r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:dcat="http://www.w3.org/ns/dcat#" xmlns:dct="http://purl.org/dc/terms/">
  <dcat:Dataset rdf:about="https://feeds.example.org/ds/water">
    <dct:title xml:lang="sk">Voda</dct:title>
  </dcat:Dataset>
</rdf:RDF>"#;
    let (rows, next) = parse_dcat_page(org, cat, &src, rdfxml, None, "sk").unwrap();
    assert!(next.is_none());
    assert_eq!(rows[0].0, "https://feeds.example.org/ds/water");

    assert!(parse_dcat_page(org, cat, &src, "[not json-ld", None, "sk").is_err());
}

#[test]
fn ckan_pages_need_success_and_give_source_scoped_ids() {
    let (org, cat) = ids();
    let a = source(
        "ckan-action",
        "a",
        "https://ckan.remote.example/api/3/action/package_search?fq=x",
        &[],
    );
    let b = source("ckan-action", "b", "https://other.example/", &[]);
    let page = json!({"success": true, "result": {"count": 7, "results": [
        common::ckan_package("same-id", "air", "Ovzdušie"),
        {"name": "only-name", "title": "Bez id"}
    ]}})
    .to_string();
    let (rows_a, count) = parse_ckan_search_page(org, cat, &a, &page, "sk").unwrap();
    assert_eq!(count, 7);
    let (rows_b, _) = parse_ckan_search_page(org, cat, &b, &page, "sk").unwrap();
    assert_eq!(rows_a[0].0, "same-id");
    assert_eq!(rows_a[1].0, "only-name");
    assert_eq!(
        rows_a[0].1.id,
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            b"https://ckan.remote.example/dataset/same-id"
        )
        .to_string()
    );
    assert_ne!(
        rows_a[0].1.id, rows_b[0].1.id,
        "two sources never share a record id"
    );
    assert_ne!(rows_a[0].1.id, "same-id");
    assert_eq!(rows_a[0].1.publisher_id, "city");
    assert_eq!(rows_a[0].1.extras["source_id"], "same-id");

    let (rows, count) = parse_ckan_search_page(org, cat, &b, r#"{"success": true}"#, "sk").unwrap();
    assert_eq!((rows.len(), count), (0, 0));
    for bad in [r#"{"success": false}"#, r#"{"result": {}}"#] {
        let err = parse_ckan_search_page(org, cat, &b, bad, "sk").unwrap_err();
        assert!(err.to_string().contains("success=false"), "{err}");
    }
    let err = parse_ckan_search_page(org, cat, &b, "<html>", "sk").unwrap_err();
    assert!(err.to_string().contains("invalid CKAN JSON"), "{err}");
}

#[test]
fn nkod_names_the_target_publisher_and_refuses_a_url_without_a_scheme() {
    let (org, cat) = ids();
    let src = with(
        source(
            "nkod-sparql",
            "nkod",
            "https://data.gov.cz/sparql",
            &["www.brno.cz"],
        ),
        |s| {
            s.target_publisher_iri = Some("https://data.gov.cz/zdroj/ovm/44992785".into());
            s.rewrite_hosts
                .insert("brno.cz".into(), "www.brno.cz".into());
        },
    );
    let rows = json!({"results": {"bindings": [
        binding(&[("ds", "https://d/a"), ("title", "A"), ("description", "Popis A"),
                  ("dist", "https://d/a/1"), ("url", "https://www.brno.cz/a.csv"), ("work", "CC0")]),
        binding(&[("ds", "https://d/b"), ("title", "B"), ("dist", "https://d/b/1"),
                  ("url", "brno.cz/b.csv"), ("work", "CC0")]),
    ]}});
    let (kept, skipped) = parse_nkod_sparql_bindings(org, cat, &src, &[], &rows, "cs");
    assert_eq!(
        reasons(&skipped),
        [(
            "https://d/b".into(),
            "all distribution URLs refused by host allowlist".into()
        )]
    );
    let ds = &kept[0].1;
    assert_eq!(ds.description["cs"], "Popis A");
    let publisher = ds.publisher.as_ref().expect("publisher");
    assert_eq!(
        publisher.uri.as_deref(),
        Some("https://data.gov.cz/zdroj/ovm/44992785")
    );
    assert_eq!(publisher.name["cs"], "city");
}

#[test]
fn a_hub_item_without_a_title_and_a_feed_dataset_without_an_iri_still_get_names() {
    let (org, cat) = ids();
    let src = source("arc-gis-hub", "hub", "https://hub.city.example", &[]);
    let item = json!({"id": "abc123", "type": "Shapefile", "license": "CC0-1.0"});
    let (kept, _) = parse_arcgis_hub_items(org, cat, &src, &[], &[item], "sk");
    assert_eq!(kept[0].1.title["sk"], "abc123");
    assert_eq!(kept[0].1.name, "city-abc123-abc123");

    let feed = source("dcat-ap", "feed", "https://feeds.example.org/c", &[]);
    let ttl = "@prefix dcat: <http://www.w3.org/ns/dcat#> .\n\
               @prefix dct: <http://purl.org/dc/terms/> .\n\
               [] a dcat:Dataset ; dct:title \"Bez IRI\"@sk .\n";
    let (rows, _) = parse_dcat_page(org, cat, &feed, ttl, Some("text/turtle"), "sk").unwrap();
    let (item, ds) = &rows[0];
    assert_eq!(
        item, &ds.name,
        "a dataset without an IRI is known by its name"
    );
    assert_eq!(
        ds.id,
        Uuid::new_v5(&Uuid::NAMESPACE_URL, item.as_bytes()).to_string()
    );
}
