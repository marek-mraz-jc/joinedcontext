//! A portal's pages built from blocks (T-7433): each block renders from a fixture manifest, a
//! portal without pages renders today's home byte for byte, and nothing a block names reaches
//! the page unchecked.

mod common;

use common::*;
use dataroom_web::*;
use serde_json::json;

/// The home of a catalogue with datasets of every state; `tests/fixtures/home_default.html` is
/// this render taken before pages existed.
fn home_fixture() -> String {
    let mut c = cat();
    c.hidden_datasets.insert("hidden".into(), "x".into());
    let mut dss: Vec<_> = (0..8)
        .map(|i| {
            dataset(
                &format!("d{i}"),
                json!({
                    "themes": if i < 5 { json!(["ENVI"]) } else { json!(["TRAN"]) },
                    "tags": if i < 3 { json!(["vzduch"]) } else { json!(["doprava"]) },
                    "modified": format!("2026-10-{:02}T00:00:00Z", 10 + i),
                    "publisherId": if i < 2 { "small" } else { "big" }
                }),
            )
        })
        .collect();
    dss.push(dataset("internal", json!({ "visibility": "internal" })));
    dss.push(dataset("hidden", json!({})));
    let pubs = [publisher("small", json!({})), publisher("big", json!({}))];
    render_home_page(&c, &dss, &pubs, "", "sk", NONCE)
}

#[test]
fn a_portal_without_pages_renders_todays_home() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/home_default.html"
    );
    let html = home_fixture();
    let want = std::fs::read_to_string(path).expect("the snapshot");
    assert!(
        html == want,
        "the default home changed; diff {path} against this render"
    );
}

const SHA_PDF: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const SHA_PNG: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const SHA_SVG: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const SHA_OTHER: &str = "4444444444444444444444444444444444444444444444444444444444444444";

fn file(sha: &str, name: &str, content_type: &str) -> dataroom_core::files::FileEntry {
    serde_json::from_value(json!({
        "id": format!("f-{}", &sha[..4]), "name": name, "contentType": content_type,
        "size": 2048, "sha256": sha, "uploadedBy": "admin",
        "uploadedAt": "2026-11-01T00:00:00Z",
    }))
    .expect("file fixture")
}

/// A branding block whose layout holds `pages`, checked by jc-core as the host checks them.
fn with_pages(pages: serde_json::Value) -> dataroom_core::BrandingBlock {
    let pages: Vec<jc_core::kinds::portal_page::PortalPage> =
        serde_json::from_value(pages).expect("pages");
    jc_core::kinds::portal_page::validate_pages(&pages).expect("valid pages");
    let mut b = branding();
    b.layout.pages = pages;
    b
}

struct Fixture {
    datasets: Vec<dataroom_core::Dataset>,
    publishers: Vec<dataroom_core::Publisher>,
    files: Vec<dataroom_core::files::FileEntry>,
    views: std::collections::BTreeMap<String, Vec<dataroom_core::ResourceView>>,
}

fn fixture() -> Fixture {
    let mut datasets: Vec<_> = (0..30)
        .map(|i| {
            dataset(
                &format!("d{i:02}"),
                json!({
                    "tags": if i % 2 == 0 { json!(["doprava"]) } else { json!(["vzduch"]) },
                    "themes": ["TRAN"],
                    "modified": format!("2026-10-{:02}T00:00:00Z", 1 + i % 28),
                    "publisherId": "mesto",
                }),
            )
        })
        .collect();
    datasets.push(dataset(
        "internal-doprava",
        json!({ "visibility": "internal", "tags": ["doprava"], "modified": "2027-01-01T00:00:00Z" }),
    ));
    datasets.push(dataset(
        "restricted-doprava",
        json!({ "visibility": "restricted", "tags": ["doprava"], "modified": "2027-01-01T00:00:00Z" }),
    ));
    datasets.push(with_dists(
        dataset(
            "zastavky",
            json!({ "modified": "2026-11-15T00:00:00Z", "publisherId": "mesto" }),
        ),
        vec![dist(
            "z1",
            &ep("zastavky/geojson"),
            json!({ "format": "GEOJSON" }),
        )],
    ));
    let mut views = std::collections::BTreeMap::new();
    views.insert(
        "d00".to_owned(),
        vec![serde_json::from_value(json!({
            "id": "v1", "resourceId": "r1", "viewType": "jc_visualization",
            "title": "Graf dopravy", "description": "x"
        }))
        .expect("view")],
    );
    Fixture {
        datasets,
        publishers: vec![publisher("mesto", json!({}))],
        files: vec![
            file(SHA_PDF, "plan mesta.pdf", "application/pdf"),
            file(SHA_PNG, "hero.png", "image/png"),
            file(SHA_SVG, "logo.svg", "image/svg+xml"),
        ],
        views,
    }
}

fn render(b: &dataroom_core::BrandingBlock, f: &Fixture, slug: &str, lang: &str) -> Option<String> {
    render_portal_page(
        &cat(),
        Some(b),
        slug,
        PageInputs {
            datasets: &f.datasets,
            publishers: &f.publishers,
            files: &f.files,
            views: &f.views,
            now: jc_time::TimestampTz::from_second(1_795_132_800).expect("now"),
        },
        "",
        lang,
        false,
        "/m",
        NONCE,
    )
}

fn every_block() -> serde_json::Value {
    json!([
        { "slug": "home", "rows": [
            { "blocks": [{ "type": "hero", "title": { "sk": "Dáta mesta", "en": "City data" },
                "intro": { "sk": "Vitajte na **portáli**." }, "image": format!("/files/{SHA_PNG}/hero.png") }] },
            { "blocks": [
                { "type": "text", "markdown": { "sk": "# Úvod\n\nText [mesto](https://mesto.sk)." } },
                { "type": "numbers", "title": { "sk": "Čísla" }, "items": ["datasets", "publishers", "updated-this-month"] }] },
            { "blocks": [{ "type": "dataset-gallery", "title": { "sk": "Doprava" }, "tag": "doprava", "limit": 3 }] },
            { "blocks": [
                { "type": "featured-datasets", "title": { "sk": "Odporúčané" }, "datasets": ["d05", "d01"] },
                { "type": "map", "title": { "sk": "Zastávky" }, "dataset": "zastavky" },
                { "type": "chart", "dataset": "d00" }] },
            { "blocks": [
                { "type": "link-cards", "title": { "sk": "Odkazy" }, "cards": [
                    { "title": { "sk": "Rozpočet" }, "text": { "sk": "Ako mesto míňa" }, "url": "/page/rozpocet" },
                    { "title": { "sk": "Mesto" }, "url": "https://mesto.sk" }] },
                { "type": "file-list", "title": { "sk": "Dokumenty" }, "files": [format!("/files/{SHA_PDF}/plan%20mesta.pdf")] }] },
        ] },
        { "slug": "rozpocet", "title": { "sk": "Rozpočet", "en": "Budget" }, "rows": [
            { "blocks": [{ "type": "text", "markdown": { "sk": "Rozpočet mesta." } }] }] },
        { "slug": "skryta", "title": { "sk": "Skrytá" }, "menu": false, "rows": [] },
    ])
}

#[test]
fn each_of_the_nine_blocks_renders_from_a_manifest() {
    let b = with_pages(every_block());
    let html = render(&b, &fixture(), "home", "sk").expect("home");
    // hero: the page's one h1, its intro through the safe renderer, its raster image.
    assert_eq!(html.matches("<h1").count(), 1, "one h1");
    assert!(html.contains("<h1>Dáta mesta</h1>"));
    assert!(html.contains("Vitajte na <strong>portáli</strong>."));
    assert!(html.contains(&format!("src=\"/m/files/{SHA_PNG}/hero.png\"")));
    assert!(html.contains("id=\"jc-hero-q-0\""), "the search box");
    // text: the leading heading is an h2, the outside link marked.
    assert!(html.contains("<h2>Úvod</h2>"));
    assert!(html.contains("<a href=\"https://mesto.sk\" rel=\"noopener nofollow\">mesto</a>"));
    // numbers: 31 public datasets (30 + zastavky), one publisher, nine since 2026-10-21.
    assert!(html.contains("<strong>31</strong>"), "public datasets only");
    assert!(html.contains("<strong>1</strong>"));
    assert!(html.contains("<strong>9</strong> <span>aktualizovaných za 30 dní"));
    // gallery: three tagged ones, newest first, and a link to the rest.
    let gallery = &html[html.find("<h2>Doprava</h2>").expect("gallery")..];
    let gallery = &gallery[..gallery.find("</section>").expect("end")];
    assert_eq!(gallery.matches("class=\"jc-card\"").count(), 3);
    assert!(gallery.contains("/m/dataset/?tags=doprava"), "{gallery}");
    // featured: in the picked order.
    let featured = &html[html.find("<h2>Odporúčané</h2>").expect("featured")..];
    assert!(
        featured.find("/dataset/d05").expect("d05") < featured.find("/dataset/d01").expect("d01")
    );
    // map: the dataset's own framable map; chart: the dataset's chart view.
    assert!(html.contains("src=\"/m/map/zastavky\""), "map");
    assert!(
        html.contains("#v1&amp;inline&amp;lang=sk") || html.contains("#v1&inline&lang=sk"),
        "chart"
    );
    assert!(html.contains("jc-viz.js"), "frames get their script");
    // link cards: a portal path under the portal's prefix, an outside link marked.
    assert!(html.contains("href=\"/m/page/rozpocet\""));
    assert!(html.contains("href=\"https://mesto.sk\" rel=\"noopener nofollow\">Mesto"));
    // file list: the store's name, type and size.
    assert!(html.contains(&format!(
        "href=\"/m/files/{SHA_PDF}/plan%20mesta.pdf\" download"
    )));
    assert!(html.contains("plan mesta.pdf") && html.contains(">PDF<") && html.contains("2 kB"));
    // the menu lists the pages that ask for it, in the reader's language.
    assert!(html.contains("<a href=\"/m/page/rozpocet\">Rozpočet</a>"));
    assert!(
        !html.contains("/m/page/skryta\""),
        "menu: false stays out of the menu"
    );
    let en = render(&b, &fixture(), "rozpocet", "en").expect("page");
    assert!(
        en.contains("<title>Budget - "),
        "a page's title in the reader's language"
    );
    assert!(en.contains("<h1 class=\"page-heading\">Budget</h1>"));
    assert!(
        en.contains("Rozpočet mesta."),
        "a text falls back to the default language"
    );
    assert!(
        render(&b, &fixture(), "nie-je", "sk").is_none(),
        "an unknown page"
    );
}

#[test]
fn a_gallery_shows_at_most_its_limit_and_never_more_than_24() {
    let f = fixture();
    let b = with_pages(json!([{ "slug": "home", "rows": [{ "blocks": [
        { "type": "dataset-gallery", "limit": 24, "layout": "table" }] }] }]));
    let html = render(&b, &f, "home", "sk").expect("home");
    assert_eq!(html.matches("<tr><td>").count(), 24, "the limit, of 31");
    // A manifest that got past the check still never shows more than 24.
    let mut b = b;
    if let jc_core::kinds::portal_page::PortalBlock::DatasetGallery(g) =
        &mut b.layout.pages[0].rows[0].blocks[0]
    {
        g.limit = 200;
    }
    let html = render(&b, &f, "home", "sk").expect("home");
    assert_eq!(html.matches("<tr><td>").count(), 24);
}

#[test]
fn a_map_or_chart_says_what_it_waits_for_and_when_there_is_nothing_to_draw() {
    let b = with_pages(json!([{ "slug": "home", "rows": [
        { "blocks": [{ "type": "map", "dataset": "zastavky" }, { "type": "chart", "dataset": "d00" }] },
        { "blocks": [{ "type": "map", "dataset": "d01" }, { "type": "chart", "dataset": "d02" }] },
    ] }]));
    let html = render(&b, &fixture(), "home", "sk").expect("home");
    // Drawable: a waiting line behind each frame, which a failed load leaves in place.
    assert_eq!(html.matches("<iframe class=\"jc-map-frame\"").count(), 1);
    assert_eq!(html.matches("<iframe class=\"jc-viz-frame\"").count(), 1);
    assert!(html.contains("Načítava sa mapa. Ak sa nezobrazí, otvorte dataset nižšie."));
    assert!(html.contains("Načítava sa graf. Ak sa nezobrazí, otvorte dataset nižšie."));
    // Nothing to draw: the block stays, says so and leads to the dataset.
    assert!(html.contains("Tento dataset nemá miesta, ktoré by sa dali ukázať na mape."));
    assert!(html.contains("Tento dataset zatiaľ nemá graf; údaje sú na stránke datasetu."));
    for name in ["zastavky", "d00", "d01", "d02"] {
        assert!(
            html.contains(&format!("<a href=\"/m/dataset/{name}\">Otvoriť dataset →</a>")),
            "{name}"
        );
    }
}

#[test]
fn an_internal_or_restricted_dataset_never_shows_in_any_block() {
    let b = with_pages(json!([{ "slug": "home", "rows": [
        { "blocks": [{ "type": "dataset-gallery", "tag": "doprava", "limit": 24 }] },
        { "blocks": [{ "type": "featured-datasets", "datasets": ["internal-doprava", "restricted-doprava"] },
                     { "type": "map", "dataset": "internal-doprava" },
                     { "type": "chart", "dataset": "restricted-doprava" }] },
        { "blocks": [{ "type": "numbers", "items": ["datasets"] }] },
    ] }]));
    let html = render(&b, &fixture(), "home", "sk").expect("home");
    assert!(!html.contains("internal-doprava") && !html.contains("restricted-doprava"));
    assert!(html.contains("<strong>31</strong>"));
}

#[test]
fn markdown_and_links_are_made_safe() {
    let mut b = with_pages(json!([{ "slug": "home", "rows": [{ "blocks": [
        { "type": "text", "markdown": { "sk": "<script>alert(1)</script>\n\n[x](javascript:alert(2)) ![a](https://evil.sk/p.png)" } },
        { "type": "hero", "title": { "sk": "Dáta" }, "intro": { "sk": "<img src=x onerror=alert(3)>" } },
        { "type": "link-cards", "cards": [{ "title": { "sk": "Ok" }, "url": "/ok" }] },
    ] }] }]));
    // A card a check never saw (a hand-edited store) still loses an unsafe link.
    if let jc_core::kinds::portal_page::PortalBlock::LinkCards(c) =
        &mut b.layout.pages[0].rows[0].blocks[2]
    {
        c.cards.push(
            serde_json::from_value(json!({
            "title": { "sk": "Zlý" }, "url": "javascript:alert(4)" }))
            .expect("card"),
        );
    }
    let html = render(&b, &fixture(), "home", "sk").expect("home");
    for bad in [
        "<script>alert(1)",
        "href=\"javascript:",
        "<img src=x",
        "evil.sk",
        ">Zlý<",
    ] {
        assert!(!html.contains(bad), "{bad}");
    }
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "raw HTML shows as text"
    );
}

#[test]
fn a_file_the_portal_does_not_hold_is_never_rendered() {
    let b = with_pages(json!([{ "slug": "home", "rows": [{ "blocks": [
        { "type": "hero", "image": format!("/files/{SHA_SVG}/logo.svg") },
        { "type": "file-list", "files": [format!("/files/{SHA_OTHER}/cudzi.pdf")] },
    ] }] }]));
    let html = render(&b, &fixture(), "home", "sk").expect("home");
    assert!(!html.contains(SHA_OTHER), "another Organization's file");
    assert!(!html.contains(SHA_SVG), "an SVG never shows inline");
    assert!(
        !html.contains("jc-block-files"),
        "a list of nothing is left out"
    );
}
