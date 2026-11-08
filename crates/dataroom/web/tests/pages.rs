//! Every rendered page: its empty and error states (C12), escaping of publisher text, the CSP
//! nonce on every script and style (D7), the language links (C2) and the STATUS claims of D1-D7.

mod common;

use common::*;
use dataroom_core::{BrandingBlock, FormFieldLayout, ShaclValidationReport};
use dataroom_search::{FacetCount, SearchFacets};
use dataroom_web::i18n::jc_t;
use dataroom_web::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const XSS: &str = "Air '\"><img src=x onerror=alert(1)><script>alert(2)</script>";

/// What every page must hold, whatever its data: the page's language, a nonce on every script
/// and style element, no inline event handler and no `javascript:` link.
fn assert_safe_page(name: &str, html: &str, lang: &str) {
    assert!(!html.is_empty(), "{name}: empty render");
    assert!(
        html.contains(&format!("<html lang=\"{lang}\"")),
        "{name}: <html lang>"
    );
    for tag in tags(html, "script").into_iter().chain(tags(html, "style")) {
        assert!(
            tag.contains(&format!("nonce=\"{NONCE}\"")),
            "{name}: un-nonced {tag}"
        );
    }
    let lower = html.to_ascii_lowercase();
    assert!(
        !lower.contains("href=\"javascript:"),
        "{name}: javascript: link"
    );
    assert!(
        !lower.contains("src=\"javascript:"),
        "{name}: javascript: src"
    );
    for (i, _) in lower.match_indices('<') {
        let tag_end = lower[i..].find('>').map_or(lower.len(), |e| i + e);
        let tag = &lower[i..tag_end];
        for attr in attribute_names(tag) {
            assert!(
                !(attr.starts_with("on") && attr.len() > 2),
                "{name}: inline handler {attr} in {tag}"
            );
        }
    }
    assert!(
        !html.contains("<img src=x"),
        "{name}: publisher HTML rendered"
    );
    assert!(
        !html.contains("<script>alert"),
        "{name}: publisher script rendered"
    );
}

/// The attribute names of one opening tag, quoted values skipped.
fn attribute_names(tag: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut in_value = false;
    for c in tag.chars().skip_while(|c| !c.is_whitespace()) {
        match quote {
            Some(q) if c == q => {
                quote = None;
                in_value = false;
            }
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '=' => {
                names.push(std::mem::take(&mut current));
                in_value = true;
            }
            None if c.is_whitespace() => {
                in_value = false;
                current.clear();
            }
            None if !in_value => current.push(c),
            None => {}
        }
    }
    names
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn esc_t(key: &str, lang: &str) -> String {
    esc(&jc_t(key, lang))
}

fn facet(value: &str, count: usize) -> FacetCount {
    FacetCount {
        value: value.into(),
        label: format!("L {value}"),
        count,
    }
}

fn hostile_dataset() -> dataroom_core::Dataset {
    with_dists(
        dataset(
            "air",
            json!({
                "title": { "sk": XSS, "en": XSS },
                "description": { "sk": XSS },
                "tags": [XSS],
                "endpointUrl": ep(""),
                "licenceUrl": "javascript:alert(3)",
                "licenceId": "custom",
                "extras": { "source": format!("[\"https://a.example/{XSS}\"]"), "note": XSS }
            }),
        ),
        vec![
            dist("csv", &ep("file.csv"), json!({ "title": { "sk": XSS } })),
            dist(
                "bad",
                "javascript:alert(4)",
                json!({ "format": "CSV", "title": { "sk": XSS } }),
            ),
            dist(
                "ogc",
                &ep("ogc/features/collections/AirQualityObserved"),
                json!({}),
            ),
            dist(
                "ws",
                &ep("ws"),
                json!({ "description": { "sk": "kľúč $TOKEN" } }),
            ),
        ],
    )
}

#[test]
fn every_page_escapes_publisher_text_and_nonces_every_script() {
    let c = catalogue(json!({ "domain": DOMAIN, "samples": { "sk": [XSS] } }));
    let ds = hostile_dataset();
    let p = publisher(
        "city",
        json!({ "title": { "sk": XSS }, "description": { "sk": XSS } }),
    );
    let facets = SearchFacets {
        keyword: vec![facet(XSS, 1)],
        ..SearchFacets::default()
    };
    let none = BTreeMap::new();
    let views = [view("c1", "jc_visualization", XSS)];
    let pages: Vec<(&str, String)> = vec![
        (
            "home",
            render_home_page(
                &c,
                std::slice::from_ref(&ds),
                std::slice::from_ref(&p),
                XSS,
                "sk",
                NONCE,
            ),
        ),
        (
            "list",
            render_dataset_list_page(
                &c,
                None,
                std::slice::from_ref(&ds),
                std::slice::from_ref(&p),
                &facets,
                1,
                1,
                XSS,
                XSS,
                &none,
                "sk",
                true,
                "",
                NONCE,
            ),
        ),
        (
            "dataset",
            render_dataset_page_full(
                &c,
                None,
                &ds,
                Some(&p),
                &views,
                &[],
                true,
                "sk",
                true,
                "",
                NONCE,
            ),
        ),
        (
            "resource",
            render_resource_page(
                &c,
                None,
                &ds,
                &ds.distributions[1],
                Some(&p),
                "sk",
                true,
                "",
                NONCE,
            ),
        ),
        (
            "resource-csv",
            render_resource_page(
                &c,
                None,
                &ds,
                &ds.distributions[0],
                Some(&p),
                "sk",
                true,
                "",
                NONCE,
            ),
        ),
        (
            "resource-ws",
            render_resource_page(
                &c,
                None,
                &ds,
                &ds.distributions[3],
                Some(&p),
                "sk",
                true,
                "",
                NONCE,
            ),
        ),
        (
            "orgs",
            render_organization_list_page(
                &c,
                None,
                std::slice::from_ref(&p),
                std::slice::from_ref(&ds),
                "",
                XSS,
                "sk",
                true,
                "",
                NONCE,
            ),
        ),
        (
            "org",
            render_organization_read_page(
                &c,
                None,
                &p,
                std::slice::from_ref(&ds),
                &facets,
                1,
                XSS,
                XSS,
                "sk",
                true,
                "",
                NONCE,
            ),
        ),
        ("about", render_about_page(&c, None, "sk", true, "", NONCE)),
        (
            "assistants",
            render_assistants_page(&c, None, "sk", true, "", NONCE),
        ),
        (
            "guide",
            render_assistant_guide_page(&c, None, "claude", "sk", true, "", NONCE).expect("guide"),
        ),
        (
            "map",
            render_map_page(&c, None, &ds, None, "sk", true, "", NONCE).expect("map"),
        ),
    ];
    for name in STATIC_PAGE_NAMES_FOR_TEST {
        let html = render_static_page(&c, None, name, "sk", true, "", NONCE).expect(name);
        assert_safe_page(name, &html, "sk");
    }
    for (name, html) in &pages {
        assert_safe_page(name, html, "sk");
    }
    let page = |n: &str| pages.iter().find(|(k, _)| *k == n).expect(n).1.clone();
    // The escaped title is still there, as text.
    assert!(page("dataset").contains("&lt;img src=x onerror=alert(1)&gt;"));
    // A `javascript:` licence or resource address is never a link.
    assert!(!page("resource").contains("alert(4)\""));
    assert!(!page("dataset").contains("alert(3)"));
}

const STATIC_PAGE_NAMES_FOR_TEST: [&str; 5] =
    ["connector", "privacy", "terms", "support", "accessibility"];

#[test]
fn map_config_in_an_attribute_cannot_end_the_attribute() {
    // data-jc-map='...' is single-quoted: a ' in a dataset title must arrive escaped, or the
    // rest of the title becomes markup (D3, D6).
    let c = cat();
    let ds = with_dists(
        dataset(
            "m",
            json!({ "title": { "sk": "x' onmouseover='alert(1)' y='" } }),
        ),
        vec![dist("ogc", &ep("ogc/features/collections/A"), json!({}))],
    );
    let map = render_map_page(&c, None, &ds, None, "sk", false, "", NONCE).expect("map");
    let full = render_dataset_page(&c, &ds, None, &[], false, "sk", NONCE);
    for (name, html) in [("map", map), ("dataset", full)] {
        let start = html
            .find("data-jc-map='")
            .unwrap_or_else(|| panic!("{name}: map element"))
            + 13;
        let value = &html[start..start + html[start..].find('\'').expect("attribute end")];
        let decoded = value
            .replace("&quot;", "\"")
            .replace("&#x27;", "'")
            .replace("&#39;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&#x2f;", "/")
            .replace("&amp;", "&");
        let cfg: Value =
            serde_json::from_str(&decoded).unwrap_or_else(|e| panic!("{name}: {e}: {decoded}"));
        assert_eq!(cfg["title"], "x' onmouseover='alert(1)' y='", "{name}");
        let outside_json_ld = match html.find("<script type=\"application/ld+json\"") {
            Some(i) => format!(
                "{}{}",
                &html[..i],
                &html[i + html[i..].find("</script>").expect("end")..]
            ),
            None => html.clone(),
        };
        if let Some(i) = outside_json_ld.find("onmouseover='alert") {
            panic!(
                "{name}: attribute broken out: {}",
                &outside_json_ld[i.saturating_sub(300)..i + 40]
            );
        }
    }
}

#[test]
fn csp_header_frames_only_the_map_and_names_the_platform_hosts() {
    let page = csp_header_for_domain("abc", Some(DOMAIN), false);
    assert!(page.contains("script-src 'self' 'nonce-abc';"));
    assert!(page.contains("style-src 'self' 'nonce-abc';"));
    assert!(page.contains("frame-ancestors 'self';"));
    assert!(page.contains(&format!(
        "connect-src 'self' https://{DOMAIN} https://*.{DOMAIN};"
    )));
    assert!(page.contains("object-src 'none'") && page.contains("base-uri 'self'"));
    assert!(!page.contains("unsafe-inline") && !page.contains("unsafe-eval"));
    let map = csp_header_for_domain("abc", Some(DOMAIN), true);
    assert!(map.contains("frame-ancestors *;"));
    for odd in ["", "evil.example; script-src *", "a b", "x'"] {
        let h = csp_header_for_domain("n", Some(odd), false);
        assert!(h.contains("connect-src 'self';"), "{odd:?}: {h}");
    }
    assert_eq!(
        csp_header("n", true),
        csp_header_for_domain("n", None, true)
    );
}

#[test]
fn odata_grid_config_points_the_sdk_grid_at_an_entity_set() {
    let g = odata_grid_config(
        "/m/odata/v4",
        "Datasets",
        "Dataset",
        &[("title", "Názov", "text")],
    );
    assert_eq!(
        g,
        json!({
            "source": { "kind": "odata", "url": "/m/odata/v4", "entitySet": "Datasets" },
            "type": "Dataset",
            "columns": [{ "attr": "title", "label": "Názov", "format": "text" }],
            "pageSize": 50, "mode": "view", "density": "comfortable", "map": { "enabled": false }
        })
    );
}

// ---- D1 home ----

#[test]
fn d1_home_counts_public_records_and_orders_tiles_recent_publishers_and_keywords() {
    let mut c = cat();
    c.hidden_datasets.insert("hidden".into(), "x".into());
    let env = "http://publications.europa.eu/resource/authority/data-theme/ENVI";
    let mut dss: Vec<_> = (0..8)
        .map(|i| {
            dataset(
                &format!("d{i}"),
                json!({
                    "themes": if i < 5 { json!([env, "ENVI"]) } else { json!(["TRAN"]) },
                    "tags": if i < 3 { json!(["vzduch", "a b&c"]) } else { json!(["doprava"]) },
                    "modified": format!("2026-10-{:02}T00:00:00Z", 10 + i),
                    "publisherId": if i < 2 { "small" } else { "big" }
                }),
            )
        })
        .collect();
    dss.push(dataset(
        "hidden",
        json!({ "modified": "2027-01-01T00:00:00Z" }),
    ));
    dss.push(dataset(
        "internal",
        json!({ "visibility": "internal", "modified": "2027-01-01T00:00:00Z" }),
    ));
    dss.push(dataset(
        "draft",
        json!({ "state": "draft", "modified": "2027-01-01T00:00:00Z" }),
    ));
    let pubs = [
        publisher("small", json!({ "description": { "sk": "x".repeat(100) } })),
        publisher("big", json!({})),
        publisher("gone", json!({ "state": "withdrawn" })),
    ];
    let html = render_home_page(&c, &dss, &pubs, "", "sk", NONCE);
    assert_safe_page("home", &html, "sk");
    // Topic tiles: ENVI (5) before TRAN (3), each theme counted once per dataset.
    let envi = html.find("vocab_jc_topic=ENVI").expect("ENVI tile");
    let tran = html.find("vocab_jc_topic=TRAN").expect("TRAN tile");
    assert!(envi < tran);
    assert!(
        html.contains("5 datasetov") && html.contains("3 datasety"),
        "plural counts"
    );
    // Recent: the newest six public ones, newest first; hidden, internal and draft never.
    let recent = &html[html.find("id=\"jc-recent-title\"").expect("recent")..];
    let recent = &recent[..recent
        .find("id=\"jc-publishers-title\"")
        .unwrap_or(recent.len())];
    assert!(recent.find("/dataset/d7").expect("d7") < recent.find("/dataset/d2").expect("d2"));
    assert!(!recent.contains("/dataset/d1\""), "only six");
    for absent in [
        "/dataset/hidden",
        "/dataset/internal",
        "/dataset/draft",
        "/organization/gone",
    ] {
        assert!(!html.contains(absent), "{absent}");
    }
    // Publishers by count: big (6) before small (2); a long description is cut at 80.
    assert!(
        html.find("organization=big").expect("big")
            < html.find("organization=small").expect("small")
    );
    // Keywords: most used first, the href encoded.
    assert!(
        html.find("tags=doprava").expect("doprava") < html.find("tags=vzduch").expect("vzduch")
    );
    assert!(html.contains("tags=a+b%26c"));
}

#[test]
fn d1_home_empty_catalogue_says_so() {
    let html = render_home_page(&cat(), &[], &[], "", "en", NONCE);
    assert_safe_page("home-empty", &html, "en");
    assert!(html.contains(&esc(&jc_t("no_datasets", "en"))));
    assert!(!html.contains("id=\"jc-topics-title\""));
    assert!(!html.contains("id=\"jc-publishers-title\""));
    assert!(!html.contains("id=\"jc-tags-title\""));
}

// ---- D2 dataset list ----

#[test]
fn d2_facets_hide_a_shared_topic_link_the_active_value_and_offer_more() {
    let mut facets = SearchFacets {
        publisher: (0..12).map(|i| facet(&format!("p{i}"), 1)).collect(),
        theme: vec![facet("ENVI", 4), facet("TRAN", 2)],
        format: vec![facet("ogcfeat", 4)],
        licence: vec![facet("cc-by", 4)],
        spatial: vec![facet("SK032", 3), facet("urn:x", 1)],
        ..SearchFacets::default()
    };
    let active = BTreeMap::from([
        ("organization".to_owned(), "p1".to_owned()),
        ("spatial".to_owned(), "SK032".to_owned()),
    ]);
    let dss = [dataset("a", json!({})), dataset("b", json!({}))];
    let html = render_dataset_list_page(
        &cat(),
        None,
        &dss,
        &[],
        &facets,
        4,
        1,
        "",
        "",
        &active,
        "en",
        false,
        "/data",
        NONCE,
    );
    assert_safe_page("list", &html, "en");
    assert!(
        !html.contains("vocab_jc_topic=ENVI"),
        "a topic every result shares is hidden"
    );
    assert!(html.contains("/data/dataset/?vocab_jc_topic=TRAN"));
    assert!(
        html.contains("/data/dataset/?_organization_limit=0"),
        "more than 10 publishers"
    );
    assert!(!html.contains("organization=p11"), "only ten shown");
    assert!(html.contains("OGC API Features"), "format label");
    assert!(html.contains("(NUTS SK032)"), "NUTS label");
    assert!(html.contains("/data/dataset/?spatial=urn%3Ax"));
    assert!(html.contains(&esc_t("datasets_found", "en").replace("{n}", "4")));
    // The active topic stays listed when the search filters by it.
    facets.theme = vec![facet("ENVI", 4)];
    let active = BTreeMap::from([("vocab_jc_topic".to_owned(), "ENVI".to_owned())]);
    let html = render_dataset_list_page(
        &cat(),
        None,
        &dss,
        &[],
        &facets,
        4,
        1,
        "",
        "",
        &active,
        "en",
        false,
        "",
        NONCE,
    );
    assert!(html.contains("L ENVI"));
}

#[test]
fn d2_list_without_results_shows_the_empty_text_and_one_page() {
    let html = render_dataset_list_page(
        &cat(),
        None,
        &[],
        &[],
        &SearchFacets::default(),
        0,
        1,
        "nič",
        "",
        &BTreeMap::new(),
        "sk",
        true,
        "",
        NONCE,
    );
    assert_safe_page("list-empty", &html, "sk");
    assert!(html.contains(&esc(&jc_t("empty", "sk"))));
    assert!(html.contains("value=\"nič\""));
    let many = render_dataset_list_page(
        &cat(),
        None,
        &[],
        &[],
        &SearchFacets::default(),
        205,
        2,
        "",
        "",
        &BTreeMap::new(),
        "sk",
        false,
        "",
        NONCE,
    );
    assert!(many.contains(&esc_t("datasets_found", "sk").replace("{n}", "205")));
}

// ---- D3 dataset page ----

#[test]
fn d3_dataset_page_shows_every_block_for_a_platform_dataset() {
    let mut c = cat();
    c.branding = Some(json!({ "domain": DOMAIN, "noindex": true }));
    let ds = with_dists(
        dataset(
            "air",
            json!({
                "endpointUrl": ep(""), "licenceId": "cc-by",
                "extras": { "source": "[\"https://a.example/1\",\"https://a.example/2\",\"https://a.example/3\",\"https://a.example/4\"]", "quality_pass_rate": "0.5" }
            }),
        ),
        vec![
            dist("csv", &ep("file.csv"), json!({})),
            dist(
                "ogc",
                &ep("ogc/features/collections/AirQualityObserved"),
                json!({}),
            ),
            dist("cal", "https://x.example/c.ics", json!({ "format": "ICS" })),
            dist(
                "feed",
                "https://x.example/f",
                json!({ "mediaType": "application/rss+xml" }),
            ),
        ],
    );
    let report: ShaclValidationReport = serde_json::from_value(json!({
        "orgId": "00000000-0000-0000-0000-000000000000",
        "catalogueId": "00000000-0000-0000-0000-000000000000",
        "id": "r", "datasetId": "air", "profileId": "dcat-ap-sk", "profileVersion": "3",
        "conforms": false, "violationCount": 2, "warningCount": 1, "findings": [],
        "checkedAt": "2026-11-01T10:00:00Z"
    }))
    .expect("report");
    let views = [view("c1", "jc_visualization", "Chart")];
    let html = render_dataset_page_full(
        &c,
        None,
        &ds,
        None,
        &views,
        &[report],
        true,
        "en",
        true,
        "/m",
        NONCE,
    );
    assert_safe_page("dataset", &html, "en");
    for id in [
        "jc-explorer-title",
        "jc-viz-title",
        "jc-about-title",
        "jc-live-title",
        "jc-mcp-title",
        "jc-resources-title",
        "jc-subscribe-calendar",
        "jc-subscribe-news",
        "jc-map-embed",
        "jc-more-title",
        "shacl-heading",
    ] {
        assert!(html.contains(&format!("id=\"{id}\"")), "{id}");
    }
    assert!(
        html.contains("<meta name=\"robots\" content=\"noindex, nofollow\">"),
        "B4 noindex"
    );
    assert!(html.contains("<td>dcat-ap-sk</td>"));
    // Four sources fold into one row naming the count.
    assert!(html.contains(&esc_t("sources_all", "en").replace("{n}", "4")));
    assert_eq!(
        html.matches("<dt>Source</dt>").count().max(
            html.matches(&format!("<dt>{}</dt>", jc_t("source", "en")))
                .count()
        ),
        1
    );
    // schema.org JSON-LD carries the nonce and names the dataset.
    assert!(html.contains(&format!(
        "<script type=\"application/ld+json\" nonce=\"{NONCE}\">"
    )));
    // C2: hreflang alternates keep the page; links carry the mount and language.
    for lang in ["sk", "en", "cs"] {
        assert!(
            html.contains(&format!(
                "hreflang=\"{lang}\" href=\"/m/{lang}/dataset/air\""
            )),
            "{lang}"
        );
    }
    assert!(html.contains("href=\"/m/en/dataset/air/resource/csv\""));
}

#[test]
fn d3_dataset_page_without_domain_or_resources_leaves_platform_blocks_out() {
    let c = catalogue(json!({}));
    let ds = dataset("bare", json!({ "endpointUrl": ep("") }));
    let html = render_dataset_page(&c, &ds, None, &[], false, "de", NONCE);
    assert_safe_page("dataset-bare", &html, "de");
    for id in [
        "jc-explorer-title",
        "jc-viz-title",
        "jc-live-title",
        "jc-mcp-title",
        "jc-map-embed",
        "shacl-heading",
    ] {
        assert!(
            !html.contains(&format!("id=\"{id}\"")),
            "{id} without a domain"
        );
    }
    let few = with_dists(
        dataset(
            "few",
            json!({ "extras": { "source": "https://a.example/1" } }),
        ),
        vec![dist(
            "x",
            "https://x.example/a.pdf",
            json!({ "format": "PDF" }),
        )],
    );
    let html = render_dataset_page(&c, &few, None, &[], true, "sk", NONCE);
    assert!(
        !html.contains("jc-about-sources"),
        "one source does not fold"
    );
    assert!(
        html.contains("id=\"shacl-heading\""),
        "editor panel without reports"
    );
}

// ---- D4 resource page ----

#[test]
fn d4_resource_page_links_only_http_and_shows_preview_examples_and_facts() {
    let c = cat();
    let ds = with_dists(
        dataset(
            "air",
            json!({ "endpointUrl": ep(""), "licenceId": "cc-by", "issued": "2026-01-01T00:00:00Z" }),
        ),
        vec![
            dist(
                "ent",
                &ep("ngsi-ld/v1/entities?type=AirQualityObserved"),
                json!({}),
            ),
            dist(
                "file",
                &ep("file.csv"),
                json!({ "modified": "2026-11-05T00:00:00Z" }),
            ),
            dist("bad", "javascript:alert(1)", json!({ "format": "CSV" })),
            dist(
                "src",
                "https://city.example/page",
                json!({ "title": { "sk": "Dataset air", "en": "Dataset air en" } }),
            ),
            dist("ws", &ep("ws"), json!({})),
            dist(
                "csvp",
                &ep("file.csv"),
                json!({ "conformsTo": ["javascript:x"] }),
            ),
        ],
    );
    let r = |i: usize, lang: &str| {
        render_resource_page(
            &c,
            None,
            &ds,
            &ds.distributions[i],
            None,
            lang,
            false,
            "",
            NONCE,
        )
    };
    let ent = r(0, "en");
    assert_safe_page("resource-ent", &ent, "en");
    assert!(
        ent.contains("id=\"jc-explorer-title\""),
        "an API resource on the platform gets the explorer"
    );
    assert!(
        ent.contains("&amp;type=AirQualityObserved") || ent.contains("&type=AirQualityObserved")
    );
    assert!(ent.contains("id=\"jc-examples-title\""));
    let file = r(1, "sk");
    assert_safe_page("resource-file", &file, "sk");
    assert!(
        file.contains(&format!("href=\"{}\"", ep("file.csv"))),
        "download link"
    );
    assert!(file.contains("RFC 4180"), "standard");
    assert!(
        file.contains("creativecommons.org/licenses/by/4.0/"),
        "licence link"
    );
    assert!(
        file.contains("5. novembra 2026") && file.contains("1. januára 2026"),
        "updated and created"
    );
    let bad = r(2, "en");
    assert_safe_page("resource-bad", &bad, "en");
    assert!(
        !bad.contains(&format!(">{}<", esc(&jc_t("download", "en")))),
        "no download button for javascript:"
    );
    let src = r(3, "en");
    assert!(src.contains(&esc(&jc_t("source_lead", "en"))));
    assert!(src.contains("href=\"https://city.example/page\""));
    let ws = r(4, "en");
    assert!(ws.contains(">websocat</span>") && ws.contains(">ngsi-ld-ws</span>"));
    let csvp = r(5, "en");
    assert!(!csvp.contains("javascript:x"));

    // A WebSocket guide link needs an https documentation base.
    let mut c2 = cat();
    c2.branding =
        Some(json!({ "domain": DOMAIN, "documentationBaseUrl": "https://docs.example/" }));
    let ws = render_resource_page(
        &c2,
        None,
        &ds,
        &ds.distributions[4],
        None,
        "en",
        false,
        "",
        NONCE,
    );
    assert!(ws.contains("href=\"https://docs.example/API/07-websocket\""));
}

#[test]
fn d4_resource_page_preview_for_a_platform_file_api() {
    let c = cat();
    let ds = with_dists(
        dataset("p", json!({})),
        vec![dist("o", &ep("odata/v4/AirQualityObserved"), json!({}))],
    );
    // No endpoint on the dataset: no explorer, so the OData preview reads five rows.
    let html = render_resource_page(
        &c,
        None,
        &ds,
        &ds.distributions[0],
        None,
        "en",
        false,
        "",
        NONCE,
    );
    assert!(html.contains("id=\"jc-rows-title\""));
    assert!(html.contains("data-jc-kind=\"ODATA\""));
    assert!(html.contains("$top=5"));
}

// ---- D5 publishers ----

#[test]
fn d5_publisher_list_filters_sorts_and_has_an_empty_state() {
    let c = cat();
    let pubs = [
        publisher(
            "alpha",
            json!({ "title": { "sk": "Alfa" }, "description": { "sk": "d".repeat(90) } }),
        ),
        publisher("beta", json!({ "title": { "sk": "Beta" } })),
        publisher("gone", json!({ "state": "withdrawn" })),
    ];
    let dss = [
        dataset("a", json!({ "publisherId": "alpha" })),
        dataset("b", json!({ "publisherId": "pub-alpha" })),
    ];
    let asc = render_organization_list_page(&c, None, &pubs, &dss, "", "", "sk", false, "", NONCE);
    assert_safe_page("orgs", &asc, "sk");
    assert!(
        asc.find("/organization/alpha").expect("alpha")
            < asc.find("/organization/beta").expect("beta")
    );
    assert!(!asc.contains("/organization/gone"));
    assert!(
        asc.contains("<strong class=\"count\">2 "),
        "counted by name and id"
    );
    assert!(asc.contains(&format!("{}...", "d".repeat(80))));
    let desc = render_organization_list_page(
        &c,
        None,
        &pubs,
        &dss,
        "",
        "title desc",
        "sk",
        false,
        "",
        NONCE,
    );
    assert!(
        desc.find("/organization/beta").expect("beta")
            < desc.find("/organization/alpha").expect("alpha")
    );
    let found =
        render_organization_list_page(&c, None, &pubs, &dss, "BET", "", "sk", false, "", NONCE);
    assert!(found.contains("/organization/beta") && !found.contains("/organization/alpha\""));
    let by_slug =
        render_organization_list_page(&c, None, &pubs, &dss, "alph", "", "sk", false, "", NONCE);
    assert!(by_slug.contains("/organization/alpha"));
    let empty =
        render_organization_list_page(&c, None, &pubs, &dss, "zzz", "", "sk", false, "", NONCE);
    assert!(empty.contains(&esc_t("organizations_found", "sk").replace("{n}", "0")));
    assert!(
        empty.contains(&esc(&jc_t("no_publishers", "sk"))),
        "C12 empty state"
    );
    assert!(!asc.contains(&esc(&jc_t("no_publishers", "sk"))));
}

#[test]
fn d5_publisher_page_lists_its_datasets_and_facets() {
    let c = cat();
    let p = publisher("city", json!({ "description": { "sk": "Mesto" } }));
    let facets = SearchFacets {
        keyword: vec![facet("air", 2), facet("a&b", 1)],
        format: vec![facet("CSV", 2)],
        publisher: vec![facet("city", 2)],
        licence: vec![facet("cc-by", 2)],
        ..SearchFacets::default()
    };
    let html = render_organization_read_page(
        &c,
        None,
        &p,
        &[dataset("a", json!({ "publisherId": "pub-city" }))],
        &facets,
        1,
        "",
        "",
        "sk",
        true,
        "",
        NONCE,
    );
    assert_safe_page("org", &html, "sk");
    assert!(html.contains("/sk/organization/city?tags=a%26b"));
    assert!(html.contains("/sk/organization/city?_tags_limit=0"));
    assert!(html.contains("/sk/organization/city?_res_format_limit=0"));
    assert!(
        html.contains("Vydavateľ city"),
        "publisher title on the dataset row"
    );
    assert!(html.contains("Mesto"));
    let empty = render_organization_read_page(
        &c,
        None,
        &p,
        &[],
        &SearchFacets::default(),
        0,
        "",
        "",
        "sk",
        false,
        "",
        NONCE,
    );
    assert!(empty.contains(&esc(&jc_t("empty", "sk"))));
}

// ---- D6 about, static pages, assistants, guide, map ----

#[test]
fn d6_about_names_the_installation_and_only_a_real_address() {
    let mut c = cat();
    c.branding = Some(
        json!({ "domain": DOMAIN, "organisation": "City of X", "instanceName": "X data", "contactEmail": "nobody" }),
    );
    let html = render_about_page(&c, None, "en", false, "/m", NONCE);
    assert_safe_page("about", &html, "en");
    assert!(html.contains(
        &esc_t("about_intro_org", "en")
            .replace("{name}", "X data")
            .replace("{org}", "City of X")
    ));
    assert!(html.contains(&format!("https://{HOST}/m/api/3/action/package_list")));
    assert!(html.contains(&format!("https://{HOST}/m/catalog.ttl")));
    assert!(
        !html.contains("<dd><a href=\"mailto:nobody"),
        "the about block lists only an address with '@'"
    );
    let mut c = catalogue(json!({ "contactEmail": "data@x.example", "instanceName": "Y" }));
    c.theme.organisation = String::new();
    let html = render_about_page(&c, None, "sk", false, "", NONCE);
    assert!(html.contains(&esc_t("about_intro", "sk").replace("{name}", "Y")));
    assert!(html.contains("data@x.example"));
}

#[test]
fn d6_static_assistant_and_guide_pages_and_their_error_states() {
    let c = cat();
    assert!(render_static_page(&c, None, "admin", "sk", false, "", NONCE).is_none());
    let connector =
        render_static_page(&c, None, "connector", "en", false, "", NONCE).expect("connector");
    assert!(
        connector.contains("id=\"jc-mcp-title\""),
        "the connector page shows the install card"
    );
    let privacy = render_static_page(&c, None, "privacy", "en", false, "", NONCE).expect("privacy");
    assert!(!privacy.contains("id=\"jc-mcp-title\""));
    assert!(render_assistant_guide_page(&c, None, "gemini", "en", false, "", NONCE).is_none());
    let no_domain = catalogue(json!({}));
    assert!(
        render_assistant_guide_page(&no_domain, None, "claude", "en", false, "", NONCE).is_none(),
        "a guide needs the connector address"
    );
    for id in ["claude", "chatgpt", "grok"] {
        let g = render_assistant_guide_page(&c, None, id, "en", false, "", NONCE).expect(id);
        assert_safe_page(id, &g, "en");
        assert!(g.contains(&format!("https://api.{DOMAIN}")), "{id}");
    }
    let a = render_assistants_page(&c, None, "en", false, "", NONCE);
    for id in [
        "/assistants/chatgpt",
        "/assistants/claude",
        "/assistants/grok",
    ] {
        assert!(a.contains(id), "{id}");
    }
    let a = render_assistants_page(&no_domain, None, "en", false, "", NONCE);
    assert_safe_page("assistants-bare", &a, "en");
    assert!(!a.contains("id=\"jc-mcp-title\""));
}

#[test]
fn d6_map_page_needs_a_platform_layer() {
    let c = cat();
    let ds = with_dists(
        dataset("m", json!({})),
        vec![dist("ogc", &ep("ogc/features/collections/A"), json!({}))],
    );
    let html = render_map_page(&c, None, &ds, Some("ogc"), "en", false, "", NONCE).expect("map");
    assert_safe_page("map", &html, "en");
    assert!(html.contains("pmtiles-4.5.0.js"));
    assert!(html.contains(&format!("href=\"https://{HOST}/dataset/m\"")));
    assert!(render_map_page(&c, None, &ds, Some("nope"), "en", false, "", NONCE).is_none());
    let no_layer = dataset("n", json!({}));
    assert!(render_map_page(&c, None, &no_layer, None, "en", false, "", NONCE).is_none());
    assert!(render_map_page(
        &catalogue(json!({})),
        None,
        &ds,
        None,
        "en",
        false,
        "",
        NONCE
    )
    .is_none());
}

// ---- B4 per-catalogue branding ----

#[test]
fn b4_noindex_reaches_every_page_but_the_framed_map() {
    let c = catalogue(json!({ "domain": DOMAIN, "noindex": true }));
    let meta = "<meta name=\"robots\" content=\"noindex, nofollow\">";
    let ds = with_dists(
        dataset("n", json!({})),
        vec![dist("ogc", &ep("ogc/features/collections/A"), json!({}))],
    );
    let p = publisher("p", json!({}));
    let none = BTreeMap::new();
    let f = SearchFacets::default();
    for (name, html) in [
        ("home", render_home_page(&c, &[], &[], "", "sk", NONCE)),
        (
            "list",
            render_dataset_list_page(
                &c,
                None,
                &[],
                &[],
                &f,
                0,
                1,
                "",
                "",
                &none,
                "sk",
                false,
                "",
                NONCE,
            ),
        ),
        (
            "dataset",
            render_dataset_page(&c, &ds, None, &[], false, "sk", NONCE),
        ),
        (
            "resource",
            render_resource_page(
                &c,
                None,
                &ds,
                &ds.distributions[0],
                None,
                "sk",
                false,
                "",
                NONCE,
            ),
        ),
        (
            "orgs",
            render_organization_list_page(&c, None, &[], &[], "", "", "sk", false, "", NONCE),
        ),
        (
            "org",
            render_organization_read_page(&c, None, &p, &[], &f, 0, "", "", "sk", false, "", NONCE),
        ),
        ("about", render_about_page(&c, None, "sk", false, "", NONCE)),
        (
            "assistants",
            render_assistants_page(&c, None, "sk", false, "", NONCE),
        ),
        (
            "guide",
            render_assistant_guide_page(&c, None, "grok", "sk", false, "", NONCE).expect("guide"),
        ),
        (
            "static",
            render_static_page(&c, None, "terms", "sk", false, "", NONCE).expect("terms"),
        ),
    ] {
        assert!(html.contains(meta), "{name}");
    }
    let open = render_home_page(&cat(), &[], &[], "", "sk", NONCE);
    assert!(!open.contains(meta));
}

#[test]
fn installation_branding_and_mount_prefix_reach_the_chrome() {
    let mut inst = BrandingBlock::default();
    inst.apply_override_json(
        &json!({ "domain": DOMAIN, "logo": "/brand/logo.svg", "favicon": "fav.ico" }),
    );
    let c = catalogue(json!(null));
    let html = render_home_page_mounted(&c, Some(&inst), &[], &[], "", "cs", true, "/data/", NONCE);
    assert_safe_page("home-mounted", &html, "cs");
    assert!(
        html.contains("href=\"/data/fav.ico\""),
        "favicon under the mount"
    );
    assert!(
        html.contains("/data/brand/logo.svg"),
        "footer logo under the mount"
    );
    assert!(
        html.contains(&format!("https://portal.{DOMAIN}/")),
        "portal link from the installation domain"
    );
    assert!(html.contains(&format!(
        "https://assistant.{DOMAIN}/d/data/widget?embed=1&amp;dock=1&amp;lang=cs"
    )));
    assert!(html.contains("hreflang=\"sk\" href=\"/data/sk/\""));
    let chrome = PageChrome::build(
        &c,
        Some(&inst),
        "cs-CZ",
        true,
        "/x",
        Some("q=1"),
        "/data/",
        "home",
        NONCE,
    );
    assert_eq!(chrome.lang, "cs");
    assert_eq!(chrome.lang_prefix, "/data/cs");
    assert_eq!(chrome.locale_root, format!("https://{HOST}/data/cs/"));
    assert_eq!(chrome.footer_pages.len(), 5);
    assert!(chrome
        .lang_options
        .iter()
        .any(|o| o.selected && o.code == "cs" && o.url == "/data/cs/x?q=1"));
}

// ---- C17 profile editor form ----

#[test]
fn profile_editor_form_escapes_every_value_and_marks_required_fields() {
    let field =
        |path: &str, name: &str, label: Value, desc: Value, required: bool| -> FormFieldLayout {
            serde_json::from_value(json!({
                "path": path, "fieldName": name, "name": label, "description": desc,
                "group": null, "order": 1, "required": required, "maxCount": null,
                "nodeKind": null, "datatype": null, "classIri": null, "allowedValues": []
            }))
            .expect("field")
        };
    assert_eq!(
        render_profile_editor_form(&[], "sk"),
        "<form method=\"post\" action=\"/api/v1/datasets\" class=\"dr-profile-form\">\n</form>\n"
    );
    let html = render_profile_editor_form(
        &[
            field(
                "dct:title\"><x>",
                "title",
                json!({ "sk": "Názov <b>" }),
                json!({ "sk": "Krátky \"názov\"" }),
                true,
            ),
            field("dct:x", "x'y", json!({}), json!({}), false),
        ],
        "sk",
    );
    assert!(html.contains("data-path=\"dct:title&quot;&gt;&lt;x&gt;\""));
    assert!(html.contains(">Názov &lt;b&gt; *</label>"));
    assert!(html.contains("type=\"text\" required aria-describedby=\"hint-title\""));
    assert!(html.contains("<small id=\"hint-title\">Krátky &quot;názov&quot;</small>"));
    assert!(
        html.contains("<label for=\"field-x&#39;y\">x&#39;y</label>"),
        "label falls back to the field name"
    );
    assert!(
        !html.contains("id=\"hint-x"),
        "no hint element without a description"
    );
    assert!(!html.contains("<x>") && !html.contains("<b>"));
}

#[test]
fn theme_and_assets_ship_dark_mode_and_no_third_party_origin() {
    // B2: the stylesheet carries a dark scheme and names no other origin.
    assert!(JC_THEME_CSS.contains("@media (prefers-color-scheme: dark)"));
    assert!(!JC_THEME_CSS.contains("http://") && !JC_THEME_CSS.contains("https://"));
    assert!(!JC_THEME_CSS.contains("@import"));
    assert!(serde_json::from_str::<Value>(JC_ENUMS_JSON).is_ok());
    assert!(
        JC_LOGO_SVG.starts_with("<svg")
            && !JC_LOGO_SVG.contains("<script")
            && !JC_LOGO_SVG.contains("href")
    );
}

#[test]
fn ui_labels_are_localized() {
    let sk = UiLabels::for_lang("sk");
    let en = UiLabels::for_lang("en");
    assert_ne!(sk.search_button, en.search_button);
    assert_eq!(UiLabels::for_lang("fi").search_button, en.search_button);
    assert_eq!(sk.empty_publishers, jc_t("no_publishers", "sk"));
}

#[test]
fn d1_home_breaks_count_and_date_ties_by_label_name_and_title() {
    // plugin.py jc_topic_tiles sorts by (-count, label); recent and publisher ties keep a fixed
    // order (name, title) so the page does not change between requests.
    let dss = vec![
        dataset(
            "zz",
            json!({ "themes": ["TRAN"], "publisherId": "b-pub", "modified": "2026-10-01T00:00:00Z" }),
        ),
        dataset(
            "aa",
            json!({ "themes": ["ENVI"], "publisherId": "a-pub", "modified": "2026-10-01T00:00:00Z" }),
        ),
    ];
    let pubs = [
        publisher("b-pub", json!({ "title": { "sk": "Beta" } })),
        publisher("a-pub", json!({ "title": { "sk": "Alfa" } })),
    ];
    let html = render_home_page(&cat(), &dss, &pubs, "", "sk", NONCE);
    assert_safe_page("home-ties", &html, "sk");
    // TRAN "Doprava" before ENVI "Životné prostredie".
    assert!(
        html.find("vocab_jc_topic=TRAN").expect("TRAN")
            < html.find("vocab_jc_topic=ENVI").expect("ENVI")
    );
    let recent = &html[html.find("id=\"jc-recent-title\"").expect("recent")..];
    assert!(recent.find("/dataset/aa").expect("aa") < recent.find("/dataset/zz").expect("zz"));
    assert!(
        html.find("organization=a-pub").expect("a") < html.find("organization=b-pub").expect("b")
    );
}
