//! View rendering, the view-HTML sanitizer and the view payload check.

use dataroom_core::{Dataset, Distribution, ResourceView};
use dataroom_plugin::{render_resource_view, sanitize_view_html, validate_view_payload};
use serde_json::{json, Value};

const NIL: &str = "00000000-0000-0000-0000-000000000000";

fn dataset() -> Dataset {
    serde_json::from_value(json!({
        "orgId": NIL, "catalogueId": NIL, "id": "d-1", "name": "air<quality>",
        "title": {"sk": "Ovzdušie"}, "description": {}, "publisherId": "city",
        "visibility": "public", "state": "active",
        "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("dataset")
}

fn dist(access_url: &str) -> Distribution {
    serde_json::from_value(json!({
        "id": "r-1", "datasetId": "d-1", "title": {"sk": "CSV"}, "accessUrl": access_url
    }))
    .expect("distribution")
}

fn view(view_type: &str, title: &str, config: Value) -> ResourceView {
    serde_json::from_value(json!({
        "id": "v-1", "resourceId": "r-1", "viewType": view_type, "title": title,
        "description": "Popis <b>tučne</b>", "config": config
    }))
    .expect("view")
}

#[test]
fn a_table_view_carries_the_grid_config_and_escapes_every_value() {
    let html = render_resource_view(
        &view("datatables_view", "Tabuľka \"1\"", json!({})),
        &dist("https://data.city.example/air.csv?a=1&b=2"),
        &dataset(),
    );
    assert!(html.starts_with("<section class=\"dr-view dr-view-table\" data-sdk-grid=\""));
    assert!(
        html.contains("&quot;entitySet&quot;:&quot;Distributions&quot;"),
        "{html}"
    );
    assert!(html.contains("<h4>Tabuľka &quot;1&quot;</h4>"), "{html}");
    assert!(
        html.contains("href=\"https://data.city.example/air.csv?a=1&amp;b=2\""),
        "{html}"
    );
    assert!(html.contains(">air&lt;quality&gt;</a>"), "{html}");
}

#[test]
fn a_chart_view_names_its_endpoint() {
    let html = render_resource_view(
        &view(
            "jc_visualization",
            "Graf",
            json!({"endpoint": "air\"><script>"}),
        ),
        &dist("https://data.city.example/air.csv"),
        &dataset(),
    );
    assert!(
        html.contains("data-endpoint=\"air&quot;&gt;&lt;script&gt;\""),
        "{html}"
    );
    assert!(
        html.contains("<p>Popis &lt;b&gt;tučne&lt;/b&gt;</p>"),
        "{html}"
    );
    let bare = render_resource_view(
        &view("jc_visualization", "Graf", json!({})),
        &dist("https://data.city.example/air.csv"),
        &dataset(),
    );
    assert!(bare.contains("data-endpoint=\"\""), "{bare}");
}

#[test]
fn an_image_view_and_an_unknown_view() {
    let img = render_resource_view(
        &view("image_view", "Mapa", json!({})),
        &dist("/files/map.png"),
        &dataset(),
    );
    assert_eq!(
        img,
        "<figure class=\"dr-view dr-view-image\"><img src=\"/files/map.png\" alt=\"Mapa\" loading=\"lazy\"/></figure>"
    );
    let other = render_resource_view(&view("text_view", "Text", json!({})), &dist(""), &dataset());
    assert_eq!(
        other,
        "<section class=\"dr-view\"><h4>Text</h4><p>Popis &lt;b&gt;tučne&lt;/b&gt;</p></section>"
    );
}

#[test]
fn a_distribution_url_with_a_script_scheme_is_never_a_link() {
    for url in [
        "javascript:alert(document.cookie)",
        "JaVaScRiPt:alert(1)",
        " java\tscript:alert(1)",
        "\u{1}javascript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "vbscript:msgbox(1)",
    ] {
        for kind in ["datatables_view", "image_view"] {
            let html = render_resource_view(&view(kind, "V", json!({})), &dist(url), &dataset());
            assert!(
                html.contains("href=\"\"") || html.contains("src=\"\""),
                "{kind} {url:?}: {html}"
            );
        }
    }
    // http(s), relative and fragment links stay.
    for url in [
        "http://a.example/x",
        "HTTPS://a.example/x",
        "/x:y",
        "x/y",
        "#top",
        "?q=a:b",
    ] {
        let html =
            render_resource_view(&view("image_view", "V", json!({})), &dist(url), &dataset());
        assert!(!html.contains("src=\"\""), "{url}: {html}");
    }
}

#[test]
fn the_sanitizer_keeps_plain_markup() {
    for ok in [
        "",
        "plain text, 3 > 2",
        "<p class=\"x\">Ahoj <strong>svet</strong><br/></p>",
        "<section data-sdk-grid='{\"a\":1}' aria-label=Grid><h4>T</h4></section>",
        "<a href=\"https://a.example/?a=1&amp;b=2\" title='x'>link</a>",
        "<img src=/files/a.png alt=\"\" loading=lazy>",
        "<table><thead><tr><th colspan=2>A</th></tr></thead></table>",
        "<A HREF=\"#top\">up</A>",
    ] {
        assert_eq!(sanitize_view_html(ok), ok, "{ok}");
    }
}

#[test]
fn the_sanitizer_escapes_every_active_or_unknown_markup() {
    for bad in [
        "<script>alert(1)</script>",
        "<SCRIPT src=//evil.example/x.js></SCRIPT>",
        "<iframe src=https://evil.example></iframe>",
        "<object data=x></object>",
        "<embed src=x>",
        "<style>body{display:none}</style>",
        "<img src=x onerror=alert(1)>",
        "<img/onerror=alert(1) src=x>",
        "<svg\tonload=alert(1)>",
        "<p\nonclick=alert(1)>x</p>",
        "<p style=\"background:url(javascript:alert(1))\">x</p>",
        "<a href=\"javascript:alert(1)\">x</a>",
        "<a href=javascript:alert(1)>x</a>",
        "<a href=\"&#106;avascript:alert(1)\">x</a>",
        "<a href=\"&#x6a;avascript&#x3a;alert(1)\">x</a>",
        "<a href=\"java\tscript:alert(1)\">x</a>",
        "<a href=\"vbscript:msgbox(1)\">x</a>",
        "<img src=\"data:image/svg+xml,<svg onload=alert(1)>\">",
        "<base href=https://evil.example/>",
        "<form action=https://evil.example><button>go</button></form>",
        "<meta http-equiv=refresh content=0;url=https://evil.example>",
        "<link rel=stylesheet href=https://evil.example/x.css>",
        "<!-- <script>alert(1)</script> -->",
        "<p title=\"unclosed>x",
        "<p",
        "<p =x>",
    ] {
        let out = sanitize_view_html(bad);
        assert!(!out.contains('<'), "{bad} -> {out}");
        assert!(out.contains("&lt;"), "{bad} -> {out}");
    }
    assert_eq!(
        sanitize_view_html("<b onmouseover='x'>\"&</b>"),
        "&lt;b onmouseover=&#39;x&#39;&gt;&quot;&amp;&lt;/b&gt;"
    );
}

#[test]
fn a_view_payload_holds_only_its_declared_fields() {
    validate_view_payload(&json!({
        "id": "v", "resource_id": "r", "view_type": "jc_visualization", "title": "t",
        "description": "", "order": 1, "jc_name": "n", "spec": {}, "source": "s", "endpoint": "e"
    }))
    .expect("jc_visualization fields");
    for vt in ["datatables_view", "image_view", "text_view", "map_view"] {
        validate_view_payload(&json!({
            "view_type": vt, "image_url": "u", "page_url": "p", "filterable": true
        }))
        .unwrap_or_else(|e| panic!("{vt}: {e}"));
    }

    let err = validate_view_payload(&json!({"view_type": "image_view", "spec": {}})).unwrap_err();
    assert!(
        err.to_string()
            .contains("unexpected view field 'spec' for view_type 'image_view'"),
        "{err}"
    );
    let err = validate_view_payload(&json!({"title": "t", "image_url": "u"})).unwrap_err();
    assert!(err.to_string().contains("view_type ''"), "{err}");
    let err = validate_view_payload(&json!(["not", "an", "object"])).unwrap_err();
    assert!(
        err.to_string().contains("payload must be a JSON object"),
        "{err}"
    );
}

#[test]
fn every_built_in_view_passes_the_sanitizer_unchanged() {
    let ds = dataset();
    let d = dist("https://data.city.example/air.csv?a=1&b=2");
    for vt in [
        "datatables_view",
        "jc_visualization",
        "image_view",
        "text_view",
    ] {
        let v = view(vt, "T <1>", json!({"endpoint": "e"}));
        let html = render_resource_view(&v, &d, &ds);
        assert_eq!(sanitize_view_html(&html), html, "{vt}");
    }
}
