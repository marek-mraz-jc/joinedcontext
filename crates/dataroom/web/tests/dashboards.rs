//! The dashboards pages (T-6790): the layout and Endpoint list are untrusted catalogue text, so a
//! slug or a type outside its rule never reaches a frame address.

mod common;

use common::*;
use dataroom_core::ResourceView;
use dataroom_web::dashboards::{
    dashboard_links, dashboard_sections, is_dashboard, reads, render_dashboard_list_page,
    render_dashboard_page,
};
use serde_json::{json, Value};

const SLUG2: &str = "aaaabbbbccccdddd";

fn board(name: &str, layout: Value, endpoints: &str) -> dataroom_core::Dataset {
    dataset(
        name,
        json!({
            "datasetType": "dashboard",
            "extras": {"jc_dashboard": layout.to_string(), "jc_endpoints": endpoints}
        }),
    )
}

fn chart_view(id: &str, jc_name: &str, endpoint: &str) -> ResourceView {
    serde_json::from_value(json!({
        "id": id, "resourceId": "r1", "viewType": "jc_visualization", "title": format!("Chart {id}"),
        "config": {"jc_name": jc_name, "endpoint": endpoint}
    }))
    .expect("view")
}

fn published(name: &str, endpoint: &str) -> dataroom_core::Dataset {
    dataset(name, json!({ "endpointUrl": endpoint }))
}

#[test]
fn a_package_is_a_dashboard_only_by_its_type() {
    assert!(is_dashboard(&board("b", json!({}), "")));
    assert!(!is_dashboard(&dataset("d", json!({}))));
}

#[test]
fn reads_matches_the_endpoint_with_or_without_a_trailing_slash() {
    let ep_url = format!("https://api.{DOMAIN}/api/endpoint/{SLUG}");
    let b = board("b", json!({}), &format!("https://x/other {ep_url}/"));
    assert!(reads(&b, &published("d", &format!("{ep_url}/"))));
    // The endpoint may come from the extras instead of the field.
    let via_extra = dataset("e", json!({ "extras": {"endpoint": ep_url} }));
    assert!(reads(&b, &via_extra));
    // No endpoint, an empty one, or one the board does not list: not read.
    assert!(!reads(&b, &dataset("n", json!({}))));
    assert!(!reads(&b, &published("s", "/")));
    assert!(!reads(
        &b,
        &published("o", "https://api.example/api/endpoint/zzzz")
    ));
    // A board without jc_endpoints reads nothing.
    assert!(!reads(&dataset("x", json!({})), &published("d", &ep_url)));
}

#[test]
fn sections_draw_charts_and_grids_and_link_the_dataset_of_their_endpoint() {
    let ep_url = format!("https://api.{DOMAIN}/api/endpoint/{SLUG}");
    let layout = json!({"pages": [
        {"title": "Air", "widgets": [
            {"view": "pm10"},
            {"grid": SLUG, "type": "AirQualityObserved"},
            {"grid": SLUG2, "type": "Weather_1"}
        ]},
        {"title": "Empty", "widgets": [{"view": "nobody"}]}
    ]});
    let b = board("b", layout, &ep_url);
    let views = [chart_view("v1", "pm10", SLUG)];
    let datasets = [
        published("air", &ep_url),
        board("other", json!({}), &ep_url),
    ];
    let s = dashboard_sections(&b, &views, &datasets, &branding(), "sk", "/sk");
    assert_eq!(
        s.len(),
        1,
        "a page with no drawable widget is dropped: {s:?}"
    );
    assert_eq!(s[0].id, "jc-dashboard-page-0");
    assert_eq!(s[0].title, "Air");
    let chart = s[0].items[0].chart.as_ref().expect("chart");
    assert_eq!(chart.id, "v1");
    assert_eq!(chart.package_link, "/sk/dataset/air#jc-preview-title");
    let g = s[0].items[1].grid.as_ref().expect("grid");
    assert_eq!(g.id, "p0w1");
    assert_eq!(
        g.src,
        format!("https://viz.{DOMAIN}/explore.html#{SLUG}&inline&lang=sk&theme=light&type=AirQualityObserved&dataset=air")
    );
    assert!(g.title.ends_with(": AirQualityObserved"), "{}", g.title);
    assert!(g.sandbox.contains("allow-popups-to-escape-sandbox"));
    // A grid whose Endpoint has no dataset gets no dataset link.
    let g2 = s[0].items[2].grid.as_ref().expect("grid 2");
    assert!(g2.src.ends_with("&type=Weather_1"), "{}", g2.src);
}

#[test]
fn sections_drop_widgets_whose_slug_or_type_breaks_the_rule() {
    let bad = [
        json!({"grid": "short", "type": "T"}),
        json!({"grid": "AAAABBBBCCCCDDDD", "type": "T"}),
        json!({"grid": "a".repeat(65), "type": "T"}),
        json!({"grid": SLUG, "type": "1Type"}),
        json!({"grid": SLUG, "type": "Ty-pe"}),
        json!({"grid": SLUG, "type": ""}),
        json!({"grid": SLUG, "type": "T".repeat(129)}),
        json!({"grid": SLUG}),
        json!({"type": "T"}),
    ];
    for w in bad {
        let b = board(
            "b",
            json!({"pages": [{"title": "p", "widgets": [w.clone()]}]}),
            "",
        );
        let s = dashboard_sections(&b, &[], &[], &branding(), "sk", "");
        assert!(s.is_empty(), "{w} drew {s:?}");
    }
    // Edge lengths of the rules are kept.
    let ok = [
        ("a".repeat(16), "T".to_owned()),
        ("a".repeat(64), "T".repeat(128)),
    ];
    for (slug, kind) in ok {
        let b = board(
            "b",
            json!({"pages": [{"widgets": [{"grid": slug, "type": kind}]}]}),
            "",
        );
        assert_eq!(
            dashboard_sections(&b, &[], &[], &branding(), "sk", "").len(),
            1
        );
    }
}

#[test]
fn a_chart_endpoint_outside_the_slug_rule_gets_no_dataset_link() {
    let ep_url = format!("https://api.{DOMAIN}/api/endpoint/{SLUG}");
    let b = board("b", json!({"pages": [{"widgets": [{"view": "pm10"}]}]}), "");
    for endpoint in ["../../evil", SLUG2] {
        let views = [chart_view("v1", "pm10", endpoint)];
        let s = dashboard_sections(
            &b,
            &views,
            &[published("air", &ep_url)],
            &branding(),
            "sk",
            "",
        );
        assert_eq!(
            s[0].items[0].chart.as_ref().expect("chart").package_link,
            ""
        );
    }
}

#[test]
fn a_broken_layout_or_no_viz_address_draws_nothing() {
    let mut b = board("b", json!({}), "");
    b.extras.insert("jc_dashboard".into(), "{not json".into());
    assert!(dashboard_sections(&b, &[], &[], &branding(), "sk", "").is_empty());
    let g = board(
        "g",
        json!({"pages": [{"widgets": [{"grid": SLUG, "type": "T"}]}]}),
        "",
    );
    let no_domain = dataroom_core::BrandingBlock::default();
    assert!(dashboard_sections(&g, &[], &[], &no_domain, "sk", "").is_empty());
}

#[test]
fn sections_cap_pages_widgets_and_title_length() {
    let widgets: Vec<Value> = (0..60)
        .map(|_| json!({"grid": SLUG, "type": "T"}))
        .collect();
    let pages: Vec<Value> = (0..60)
        .map(|_| json!({"title": "x".repeat(300), "widgets": widgets}))
        .collect();
    let b = board("b", json!({ "pages": pages }), "");
    let s = dashboard_sections(&b, &[], &[], &branding(), "sk", "");
    assert_eq!(s.len(), 50);
    assert_eq!(s[0].items.len(), 50);
    assert_eq!(s[0].title.chars().count(), 200);
}

#[test]
fn the_list_page_sorts_by_title_and_names_the_publisher() {
    let pubs = [publisher("city", json!({ "id": "city" }))];
    let mut zeta = board("zeta", json!({}), "");
    zeta.title = serde_json::from_value(json!({"sk": "Zeta"})).expect("title");
    let mut alpha = board("alpha", json!({}), "");
    alpha.title = serde_json::from_value(json!({"sk": "alfa"})).expect("title");
    let mut untitled = board("untitled", json!({}), "");
    untitled.title = serde_json::from_value(json!({})).expect("title");
    untitled.publisher_id = "nobody".into();
    let html = render_dashboard_list_page(
        &cat(),
        None,
        &[zeta, alpha, untitled],
        &pubs,
        "sk",
        false,
        "",
        NONCE,
    );
    let a = html.find("/dashboards/alpha").expect("alpha card");
    let u = html.find("/dashboards/untitled").expect("untitled card");
    let z = html.find("/dashboards/zeta").expect("zeta card");
    assert!(
        a < u && u < z,
        "cards sort by lowercase title, the name standing in"
    );
    assert!(html.contains("Vydavateľ city"), "publisher title shown");
    let empty = render_dashboard_list_page(&cat(), None, &[], &[], "en", true, "", NONCE);
    assert!(empty.contains("jc-dashboards-empty"), "empty state drawn");
}

#[test]
fn the_dashboard_page_lists_only_the_datasets_it_reads() {
    let ep_url = format!("https://api.{DOMAIN}/api/endpoint/{SLUG}");
    let b = board(
        "b",
        json!({"pages": [{"title": "Air", "widgets": [{"grid": SLUG, "type": "T"}]}]}),
        &ep_url,
    );
    let mut untitled = published("untitled", &ep_url);
    untitled.title = serde_json::from_value(json!({})).expect("title");
    let datasets = [
        published("air", &ep_url),
        untitled,
        published("water", "https://api.example/api/endpoint/zzzzzzzzzzzzzzzz"),
        board("board2", json!({}), &ep_url),
    ];
    let pubs = [publisher("city", json!({ "id": "city" }))];
    let html = render_dashboard_page(
        &cat(),
        None,
        &b,
        &[],
        &datasets,
        &pubs,
        "en",
        true,
        "",
        NONCE,
    );
    assert!(html.contains("/en/dataset/air\""), "{html}");
    assert!(html.contains("/en/dataset/untitled\""));
    assert!(!html.contains("/dataset/water"));
    assert!(!html.contains("/dataset/board2"));
    assert!(
        html.contains("Dataset b en"),
        "title in the reader's language"
    );
    assert!(html.contains("Publisher city"));
    assert!(html.contains("jc-dashboard-page-0"));
    let mut bare = board("bare", json!({}), "");
    bare.title = serde_json::from_value(json!({})).expect("title");
    let empty = render_dashboard_page(&cat(), None, &bare, &[], &[], &[], "sk", false, "", NONCE);
    assert!(empty.contains("jc-dashboards-empty"), "empty state drawn");
    assert!(!empty.contains("Dataset bare"), "no title text");
    assert!(
        empty.contains("<title>bare - "),
        "name stands in for a title"
    );
}

#[test]
fn dataset_pages_link_the_dashboards_that_read_them() {
    let ep_url = format!("https://api.{DOMAIN}/api/endpoint/{SLUG}");
    let ds = published("air", &ep_url);
    let mut untitled = board("plain", json!({}), &ep_url);
    untitled.title = serde_json::from_value(json!({})).expect("title");
    let boards = [
        board("b1", json!({}), &ep_url),
        board("b2", json!({}), "https://elsewhere"),
        dataset("not-a-board", json!({"extras": {"jc_endpoints": ep_url}})),
        untitled,
    ];
    let links = dashboard_links(&ds, &boards, "sk", "sk", "/sk");
    let got: Vec<(&str, &str)> = links
        .iter()
        .map(|l| (l.title.as_str(), l.url.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("Dataset b1", "/sk/dashboards/b1"),
            ("plain", "/sk/dashboards/plain")
        ]
    );
}
