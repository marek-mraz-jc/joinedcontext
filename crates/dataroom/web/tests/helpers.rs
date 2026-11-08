//! The template helpers against what `ours/ckan-theme/plugin.py` does for the same input:
//! missing fields, unknown languages, odd formats, publisher text with HTML or shell in it, and
//! addresses off the platform or with a scheme other than http(s).

mod common;

use common::*;
use dataroom_core::BrandingBlock;
use dataroom_web::helpers::*;
use dataroom_web::i18n::jc_t;
use serde_json::{json, Value};

const LANGS: [&str; 5] = ["sk", "cs", "en", "hu", "de"];

// ---- is_url, sh, with_params (plugin.py `_is_url`, `_sh`, `_with`) ----

#[test]
fn is_url_takes_only_absolute_http_addresses_without_quotes_or_spaces() {
    for ok in [
        "https://a.example/x?y=1",
        "http://a.example",
        "https://a/b#c",
    ] {
        assert!(is_url(ok), "{ok}");
    }
    for bad in [
        "",
        "javascript:alert(1)",
        "JAVASCRIPT:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "ftp://a.example/x",
        "//a.example/x",
        " https://a.example",
        "https://a.example/a b",
        "https://a.example/\"onmouseover=x",
        "https://a.example/<b>",
        "https://a.example/{x}",
        "https://a.example/a|b",
        "https://a.example/a\\b",
        "https://a.example/a^b",
        "https://a.example/a`b",
        "https://a.example/\ttab",
    ] {
        assert!(!is_url(bad), "{bad:?} must not be a link");
    }
}

#[test]
fn shlex_quote_and_sh_match_python_shlex() {
    // shlex.quote: safe words stay bare, the rest in single quotes with each ' closed and escaped.
    assert_eq!(shlex_quote(""), "''");
    assert_eq!(shlex_quote("abc-1_2@x%+=:,./"), "abc-1_2@x%+=:,./");
    assert_eq!(shlex_quote("https://a/b?c=1"), "'https://a/b?c=1'");
    assert_eq!(shlex_quote("it's"), "'it'\"'\"'s'");
    // _sh: always one quoted word.
    assert_eq!(sh(""), "''");
    assert_eq!(sh("abc"), "'abc'");
    assert_eq!(sh("a'b"), "'a'\"'\"'b'");
}

#[test]
fn sh_keeps_every_hostile_address_one_shell_word() {
    for nasty in [
        "https://a.example/x';echo INJECTED;'",
        "https://a.example/$(echo INJECTED)",
        "https://a.example/`echo INJECTED`",
        "https://a.example/x\" ; echo INJECTED ; \"",
        "https://a.example/a b\\c",
        "'",
        "''",
    ] {
        assert_eq!(shell_words(&sh(nasty)), vec![nasty.to_owned()], "{nasty}");
        assert_eq!(shell_words(&shlex_quote(nasty)), vec![nasty.to_owned()]);
    }
}

#[test]
fn with_params_sets_parameters_and_keeps_the_others_like_plugin_with() {
    assert_eq!(
        with_params("https://x/e?type=A", &[("limit", "5")]),
        "https://x/e?type=A&limit=5"
    );
    assert_eq!(
        with_params("https://x/e", &[("$top", "5")]),
        "https://x/e?$top=5"
    );
    // An existing value is replaced, not doubled.
    assert_eq!(
        with_params("https://x/e?limit=100&q=a", &[("limit", "5")]),
        "https://x/e?q=a&limit=5"
    );
    // parse_qs groups a repeated key where it first appeared.
    assert_eq!(
        with_params("https://x/e?a=1&b=2&a=3", &[("c", "4")]),
        "https://x/e?a=1&a=3&b=2&c=4"
    );
    // urlencode(quote_via=quote, safe="$"): space, '/', ':' and '=' are escaped, '$' is not.
    assert_eq!(
        with_params("https://x/e?q=isPartOf==urn:x/y+z", &[("$top", "5")]),
        "https://x/e?q=isPartOf%3D%3Durn%3Ax%2Fy%20z&$top=5"
    );
    // A blank value is kept, the fragment stays at the end.
    assert_eq!(
        with_params("https://x/e?a=&b=1#frag", &[("limit", "5")]),
        "https://x/e?a=&b=1&limit=5#frag"
    );
    assert_eq!(with_params("https://x/e", &[]), "https://x/e");
}

// ---- jc_live, jc_explorer, jc_developer_page ----

fn live_ds(endpoint: &str) -> dataroom_core::Dataset {
    dataset("air-quality", json!({ "endpointUrl": endpoint }))
}

#[test]
fn jc_live_shows_only_an_https_address_on_the_platform_domain() {
    let b = branding();
    assert!(
        jc_live(&dataset("x", json!({})), &b).is_none(),
        "no endpoint"
    );
    assert!(jc_live(&live_ds(&ep("").replace("https", "http")), &b).is_none());
    for off in [
        "https://kube-dns.kube-system.svc.cluster.local/x/",
        "https://dev.joinedcontext.com.evil.example/x/",
        "https://evildev.joinedcontext.com/x/",
        "javascript:alert(1)",
    ] {
        assert!(jc_live(&live_ds(off), &b).is_none(), "{off}");
    }
    let Some(live) = jc_live(&live_ds(ep("").trim_end_matches('/')), &b) else {
        panic!("an Endpoint on api.{DOMAIN} is shown");
    };
    assert_eq!(live.url, ep(""));
    assert_eq!(
        live.example,
        format!(
            "curl -H 'Accept: application/ld+json' '{}ngsi-ld/v1/types'",
            ep("")
        )
    );
    assert!(live.url_breakable.contains("<span class=\"jc-nb\">"));
    // The extra jcctl writes is read when the record has no endpointUrl.
    let from_extra = dataset("x", json!({ "extras": { "endpoint": ep("") } }));
    assert!(jc_live(&from_extra, &b).is_some());
}

#[test]
fn jc_live_without_a_platform_domain_shows_nothing() {
    // plugin.py `_on_platform`: never without a domain. A record's endpoint is the publisher's
    // text and must not point a reader at any host when the installation names no domain.
    let ds = live_ds("https://internal-admin.local/api/");
    assert!(jc_live(&ds, &BrandingBlock::default()).is_none());
    assert!(jc_live(&live_ds(&ep("")), &BrandingBlock::default()).is_none());
}

#[test]
fn jc_live_example_keeps_a_quote_in_the_address_inside_one_word() {
    let ds = live_ds(&ep("x';echo INJECTED;'"));
    let live = jc_live(&ds, &branding()).expect("on platform");
    let cmd = live
        .example
        .strip_prefix("curl -H 'Accept: application/ld+json' ")
        .expect("curl prefix");
    assert_eq!(
        shell_words(cmd),
        vec![format!("{}ngsi-ld/v1/types", live.url)]
    );
}

#[test]
fn jc_explorer_frames_viz_on_the_platform_with_checked_slug_type_and_name() {
    let b = branding();
    let odata = "/odata/v4";
    let ds = live_ds(&ep(""));
    let Some(ex) = jc_explorer(&ds, None, &b, "sk", odata) else {
        panic!("explorer for an api.{DOMAIN} Endpoint");
    };
    assert_eq!(ex.origin, format!("https://viz.{DOMAIN}"));
    assert_eq!(
        ex.src,
        format!("https://viz.{DOMAIN}/explore.html#{SLUG}&inline&lang=sk&theme=light&dataset=air-quality")
    );
    assert_eq!(ex.title, jc_t("explorer", "sk"));
    let grid: Value = serde_json::from_str(&ex.grid_config).expect("grid JSON");
    assert_eq!(grid["source"]["kind"], "odata");
    assert_eq!(grid["source"]["url"], odata);
    assert_eq!(grid["source"]["entitySet"], "Distributions");

    // A resource page adds its entity type; a type that is not a name never reaches the hash.
    let typed = dist(
        "r1",
        &ep("ngsi-ld/v1/entities?type=AirQualityObserved"),
        json!({}),
    );
    let ex = jc_explorer(&ds, Some(&typed), &b, "cs_CZ", odata).expect("explorer");
    assert!(ex
        .src
        .ends_with("&lang=cs&theme=light&dataset=air-quality&type=AirQualityObserved"));
    let hostile = dist(
        "r2",
        &ep("ngsi-ld/v1/entities?type=A%26dataset%3Devil"),
        json!({}),
    );
    let ex = jc_explorer(&ds, Some(&hostile), &b, "en", odata).expect("explorer");
    assert!(!ex.src.contains("evil"), "{}", ex.src);
    // A dataset name outside [a-z0-9_-]{2,100} is left out.
    let odd = dataset("Air Quality&x=1", json!({ "endpointUrl": ep("") }));
    let ex = jc_explorer(&odd, None, &b, "en", odata).expect("explorer");
    assert!(!ex.src.contains("dataset="), "{}", ex.src);
}

#[test]
fn jc_explorer_and_developer_page_need_an_api_endpoint_address() {
    let b = branding();
    for url in [
        format!("https://portal.{DOMAIN}/api/endpoint/{SLUG}/"),
        format!("https://api.{DOMAIN}/api/endpoint/short/"),
        format!("https://api.{DOMAIN}/api/endpoint/{}/", SLUG.to_uppercase()),
        format!("https://api.{DOMAIN}/other/{SLUG}/"),
        format!("https://api.{DOMAIN}/api/endpoint/{}/", "a".repeat(65)),
    ] {
        let ds = live_ds(&url);
        assert!(jc_explorer(&ds, None, &b, "en", "/o").is_none(), "{url}");
        assert!(jc_developer_page(&ds, &b, "en").is_none(), "{url}");
    }
    let ds = live_ds(&ep(""));
    assert!(jc_explorer(&ds, None, &BrandingBlock::default(), "en", "/o").is_none());
    assert!(jc_developer_page(&ds, &BrandingBlock::default(), "en").is_none());
    // A domain outside [a-z0-9.-] never becomes an origin.
    let odd = BrandingBlock {
        domain: "Dev.Example".into(),
        ..BrandingBlock::default()
    };
    let ds_odd = live_ds(&format!("https://api.Dev.Example/api/endpoint/{SLUG}/"));
    assert!(jc_explorer(&ds_odd, None, &odd, "en", "/o").is_none());
    assert_eq!(
        jc_developer_page(&ds, &b, "de-AT").as_deref(),
        Some(format!("https://portal.{DOMAIN}/d/{SLUG}?lang=de").as_str())
    );
}

// ---- jc_preview ----

#[test]
fn jc_preview_reads_five_rows_from_platform_addresses_only() {
    let b = branding();
    let p = |url: &str| jc_preview(&dist("r", url, json!({})), &b, "en");
    let entities = p(&ep("ngsi-ld/v1/entities?type=A")).expect("entities");
    assert_eq!(
        entities.url,
        ep("ngsi-ld/v1/entities?type=A&limit=5&options=keyValues&count=true")
    );
    assert_eq!(entities.kind, "NGSI-LD");
    assert_eq!(entities.heading, jc_t("preview_api", "en"));
    // Without a query the parameters start with '?' (the address stays valid).
    assert_eq!(
        p(&ep("ngsi-ld/v1/entities")).expect("entities").url,
        ep("ngsi-ld/v1/entities?limit=5&options=keyValues&count=true")
    );
    let types = p(&ep("ngsi-ld/v1/?x=1")).expect("types");
    assert_eq!(types.url, ep("ngsi-ld/v1/types"));
    assert_eq!(types.kind, "NGSI-LD-TYPES");
    assert_eq!(types.heading, jc_t("preview_types", "en"));
    let odata = p(&ep("odata/v4/AirQuality")).expect("odata");
    assert_eq!(odata.url, ep("odata/v4/AirQuality?$top=5"));
    assert_eq!(odata.kind, "ODATA");
    assert!(p(&ep("odata/v4/")).is_none(), "OData without an entity set");
    let sta = p(&ep("sta/v1.1/Things?$filter=x")).expect("sta");
    assert_eq!(sta.url, ep("sta/v1.1/Things?$filter=x&$top=5"));
    let ogc = p(&ep("ogc/features/collections/A/items")).expect("ogc");
    assert_eq!(ogc.url, ep("ogc/features/collections/A/items?limit=5"));
    assert!(p(&ep("ogc/features/collections/A")).is_none());
    assert!(p(&ep("mcp")).is_none(), "MCP has no rows");
    assert!(p("https://example.org/data.csv").is_none(), "no kind");
    assert!(p(&ep("file.csv").replace("https", "http")).is_none());
    assert!(p("https://api.other.example/api/endpoint/x/ngsi-ld/v1/").is_none());
}

#[test]
fn jc_preview_without_a_platform_domain_fetches_nothing() {
    let d = dist(
        "r",
        "https://10.0.0.5/api/endpoint/x/ngsi-ld/v1/entities",
        json!({}),
    );
    assert!(jc_preview(&d, &BrandingBlock::default(), "en").is_none());
}

// ---- jc_examples ----

fn codes(dist_url: &str, patch: Value) -> Vec<ResourceExample> {
    jc_examples(&dist("res-1", dist_url, patch), "en")
}

#[test]
fn jc_examples_match_plugin_commands_per_kind() {
    assert!(
        codes("https://example.org/x.csv", json!({})).is_empty(),
        "no kind"
    );
    assert!(codes(&ep("schema/model.schema.json"), json!({})).is_empty());
    assert!(jc_examples(&dist("r", "", json!({})), "en").is_empty());

    let ex = codes(&ep("ngsi-ld/v1/?q=1"), json!({}));
    assert_eq!(ex.len(), 1);
    assert_eq!(ex[0].title, jc_t("ex_types", "en"));
    assert_eq!(
        ex[0].code.as_deref(),
        Some(
            format!(
                "curl -H 'Accept: application/ld+json' '{}'",
                ep("ngsi-ld/v1/types")
            )
            .as_str()
        )
    );
    assert_eq!(ex[0].id, "jc-ex-res-1-0");
    assert!(ex[0].code_breakable.contains("jc-nb"));

    let ex = codes(&ep("ngsi-ld/v1/entities?type=A"), json!({}));
    assert_eq!(
        ex[0].code.as_deref(),
        Some(
            format!(
                "curl -H 'Accept: application/ld+json' '{}'",
                ep("ngsi-ld/v1/entities?type=A&limit=5")
            )
            .as_str()
        )
    );

    let ex = codes(&ep("odata/v4/A"), json!({}));
    assert_eq!(ex.len(), 2);
    assert_eq!(
        ex[0].code.as_deref(),
        Some(format!("curl '{}'", ep("odata/v4/A?$top=5")).as_str())
    );
    assert_eq!(ex[1].code.as_deref(), Some(ep("odata/v4/A").as_str()));
    assert_eq!(ex[1].id, "jc-ex-res-1-1");
    assert_eq!(codes(&ep("sta/v1.1/Things"), json!({})).len(), 1);

    let ex = codes(&ep("ogc/features/collections/A/items"), json!({}));
    assert_eq!(
        ex[0].code.as_deref(),
        Some(format!("curl '{}'", ep("ogc/features/collections/A/items?limit=5")).as_str())
    );
    assert_eq!(ex[1].code.as_deref(), Some(ep("ogc/features/").as_str()));

    let ex = codes(&ep("mcp"), json!({}));
    assert_eq!(ex.len(), 3);
    assert_eq!(ex[0].steps.len(), 2);
    assert_eq!(ex[1].steps, vec![jc_t("ex_chatgpt", "en")]);
    assert_eq!(
        ex[2].code.as_deref(),
        Some(
            format!(
                "claude mcp add --transport http joinedcontext {}",
                ep("mcp")
            )
            .as_str()
        )
    );

    for (file, ext) in [
        ("file.csv", "csv"),
        ("file.json", "json"),
        ("file.geojson", "geojson"),
        ("file.xlsx", "xlsx"),
        ("file.parquet", "parquet"),
    ] {
        let ex = codes(&ep(file), json!({}));
        assert_eq!(
            ex[0].code.as_deref(),
            Some(format!("curl -L -o data.{ext} '{}'", ep(file)).as_str())
        );
    }
    assert!(codes(&ep(""), json!({})).is_empty(), "DCAT has no example");
}

#[test]
fn jc_examples_websocket_message_is_valid_json_with_the_type_escaped() {
    let ws = ep("ws").replace("https://", "wss://");
    let ex = codes(&ws, json!({}));
    let code = ex[0].code.clone().expect("code");
    let (cmd, msg) = code.split_once('\n').expect("two lines");
    assert_eq!(cmd, format!("websocat --protocol ngsi-ld-ws '{ws}'"));
    assert_eq!(
        msg,
        r#"{"metadata":{"x-op":"x-subscribe","x-id":"s1"},"body":{"type":"Subscription","watchedAttributes":["location"]}}"#
    );
    // A type from the address is the publisher's text: a quote stays inside the JSON string.
    let typed = codes(&format!("{ws}?type=Ve%22hicle%22%7D"), json!({}));
    let code = typed[0].code.clone().expect("code");
    let msg: Value = serde_json::from_str(code.split_once('\n').expect("two lines").1)
        .expect("the message parses as JSON");
    assert_eq!(msg["body"]["entities"][0]["type"], "Ve\"hicle\"}");
    assert_eq!(msg["metadata"]["x-op"], "x-subscribe");
}

#[test]
fn jc_examples_keep_a_hostile_address_one_shell_argument() {
    let url = ep("file.csv?x=';echo INJECTED;'");
    let ex = codes(&url, json!({}));
    let code = ex[0].code.clone().expect("code");
    let args = code.strip_prefix("curl").expect("curl");
    assert_eq!(
        shell_words(args),
        vec!["-L".to_owned(), "-o".into(), "data.csv".into(), url.clone()]
    );
    let odata = ep("odata/v4/A?$filter=a eq 'b'");
    let ex = codes(&odata, json!({}));
    let words = shell_words(
        ex[0]
            .code
            .as_deref()
            .expect("code")
            .strip_prefix("curl")
            .expect("curl"),
    );
    assert_eq!(words.len(), 1, "{words:?}");
    let mcp = ep("mcp?x=';echo INJECTED;'");
    let ex = codes(&mcp, json!({}));
    let claude = ex[2].code.clone().expect("code");
    let words = shell_words(claude.strip_prefix("claude").expect("claude"));
    assert_eq!(words.last(), Some(&mcp));
    assert!(!words.contains(&"INJECTED".to_owned()));
}

#[test]
fn jc_examples_send_the_token_header_for_a_non_public_address() {
    let patch = json!({ "description": { "en": "Send your key as $TOKEN." } });
    let ex = codes(&ep("odata/v4/A"), patch.clone());
    assert_eq!(
        ex[0].code.as_deref(),
        Some(
            format!(
                "curl -H \"Authorization: Bearer $TOKEN\" '{}'",
                ep("odata/v4/A?$top=5")
            )
            .as_str()
        )
    );
    assert_eq!(
        ex[1].code.as_deref(),
        Some(ep("odata/v4/A").as_str()),
        "Excel URL has no header"
    );
    let ws = codes(&ep("ws"), patch.clone());
    assert!(ws[0]
        .code
        .as_deref()
        .expect("code")
        .starts_with("websocat --protocol ngsi-ld-ws -H \"Authorization: Bearer $TOKEN\" '"));
    let file = codes(&ep("file.csv"), patch.clone());
    assert!(file[0]
        .code
        .as_deref()
        .expect("code")
        .starts_with("curl -H \"Authorization: Bearer $TOKEN\" -L -o data.csv"));
    for path in [
        "ngsi-ld/v1/",
        "ngsi-ld/v1/entities",
        "ogc/features/collections/A/items",
    ] {
        let ex = codes(&ep(path), patch.clone());
        assert!(
            ex[0]
                .code
                .as_deref()
                .expect("code")
                .contains("Bearer $TOKEN"),
            "{path}"
        );
    }
}

#[test]
fn jc_examples_ids_carry_only_safe_characters() {
    let ex = jc_examples(&dist("a b/<x>\"", &ep("file.csv"), json!({})), "en");
    assert_eq!(ex[0].id, "jc-ex-abx-0");
    let ex = jc_examples(&dist("<>", &ep("file.csv"), json!({})), "en");
    assert_eq!(ex[0].id, "jc-ex-resource-0");
}

// ---- jc_map_config ----

#[test]
fn jc_map_config_reads_only_platform_ogc_layers() {
    let c = cat();
    let b = branding();
    let ogc = ep("ogc/features/collections/AirQualityObserved");
    let ds = with_dists(dataset("air", json!({})), vec![dist("r1", &ogc, json!({}))]);
    let m = jc_map_config(&c, &ds, None, &b, "sk").expect("map");
    assert_eq!(m["layers"][0]["type"], "AirQualityObserved");
    assert_eq!(m["layers"][0]["items"], format!("{ogc}/items"));
    assert_eq!(m["layers"][0]["schema"], format!("{ogc}/schema"));
    assert_eq!(
        m["layers"][0]["tiles"],
        format!("{ogc}/tiles/WebMercatorQuad")
    );
    assert_eq!(
        m["basemap"]["style"],
        format!("https://tiles.{DOMAIN}/style.json")
    );
    assert_eq!(m["pmtiles"], "/pmtiles-4.5.0.js");
    assert_eq!(m["lang"], "sk");
    assert_eq!(m["title"], "Dataset air");
    assert_eq!(m["dataset"], format!("https://{HOST}/dataset/air"));
    assert_eq!(m["embed"], format!("https://{HOST}/map/air"));
    let t = m["t"].as_object().expect("labels");
    assert_eq!(t.len(), 11);
    for (k, v) in t {
        assert!(
            v.as_str().is_some_and(|s| !s.is_empty() && s != k),
            "{k} untranslated"
        );
    }
    assert!(jc_map_config(&c, &ds, Some("r1"), &b, "sk").is_some());
    assert!(jc_map_config(&c, &ds, Some("other"), &b, "sk").is_none());

    // A GeoJSON distribution on the platform is a layer too; a type that is no name is "Feature".
    let geo = ep("ogc/features/collections/Bad-Type");
    let ds2 = with_dists(
        dataset("g", json!({})),
        vec![dist("r", &geo, json!({ "format": "GeoJSON" }))],
    );
    assert_eq!(
        jc_map_config(&c, &ds2, None, &b, "en").expect("map")["layers"][0]["type"],
        "Feature"
    );

    let none = with_dists(
        dataset("n", json!({})),
        vec![dist("r", &ep("file.csv"), json!({}))],
    );
    assert!(
        jc_map_config(&c, &none, None, &b, "sk").is_none(),
        "no OGC layer"
    );
}

#[test]
fn jc_map_config_never_points_a_browser_at_another_host() {
    let c = cat();
    let off = "https://grafana.internal.example/api/endpoint/x/ogc/features/collections/A";
    let ds = with_dists(
        dataset("a", json!({})),
        vec![dist("r", off, json!({ "format": "OGCFEAT" }))],
    );
    assert!(jc_map_config(&c, &ds, None, &branding(), "sk").is_none());
    let plat = ep("ogc/features/collections/A");
    let ds = with_dists(dataset("a", json!({})), vec![dist("r", &plat, json!({}))]);
    assert!(
        jc_map_config(&c, &ds, None, &BrandingBlock::default(), "sk").is_none(),
        "no domain: no map"
    );
    let http = plat.replace("https://", "http://");
    let ds = with_dists(dataset("a", json!({})), vec![dist("r", &http, json!({}))]);
    assert!(jc_map_config(&c, &ds, None, &branding(), "sk").is_none());
}

// ---- jc_standard, jc_model, standard_display_name ----

#[test]
fn jc_standard_names_the_kind_standard_and_refuses_non_http_values() {
    for (path, url, name) in [
        (
            "file.csv",
            "https://www.rfc-editor.org/rfc/rfc4180",
            "CSV (RFC 4180)",
        ),
        (
            "file.geojson",
            "https://www.rfc-editor.org/rfc/rfc7946",
            "GeoJSON (RFC 7946)",
        ),
        (
            "file.json",
            "https://www.rfc-editor.org/rfc/rfc8259",
            "JSON (RFC 8259)",
        ),
        (
            "ngsi-ld/v1/",
            "https://www.etsi.org/deliver/etsi_gs/CIM/001_099/009/",
            "ETSI NGSI-LD (GS CIM 009)",
        ),
        (
            "ogc/features/",
            "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core",
            "OGC API - Features",
        ),
        (
            "odata/v4/",
            "https://docs.oasis-open.org/odata/odata/v4.01/",
            "OData v4",
        ),
        (
            "mcp",
            "https://modelcontextprotocol.io/specification",
            "Model Context Protocol",
        ),
    ] {
        assert_eq!(
            jc_standard(&dist("r", &ep(path), json!({}))),
            Some((url.to_owned(), name.to_owned())),
            "{path}"
        );
    }
    assert_eq!(jc_standard(&dist("r", &ep("ws"), json!({}))), None);
    let named = |c: &str| jc_standard(&dist("r", &ep("ws"), json!({ "conformsTo": [c] })));
    assert_eq!(
        named("https://www.w3.org/TR/json-ld11/"),
        Some((
            "https://www.w3.org/TR/json-ld11/".into(),
            "JSON-LD 1.1".into()
        ))
    );
    assert_eq!(
        named("https://example.org/spec"),
        Some((
            "https://example.org/spec".into(),
            "https://example.org/spec".into()
        ))
    );
    assert_eq!(named("javascript:alert(1)"), None);
    assert_eq!(named("https://example.org/a b"), None);
    assert_eq!(
        standard_display_name("https://w3.org/TR/vocab-dcat-3/"),
        "DCAT"
    );
    assert_eq!(
        standard_display_name("https://smartdatamodels.org/x"),
        "Smart Data Models"
    );
}

#[test]
fn jc_model_prefers_a_smart_data_model_then_the_json_schema() {
    let sdm = dist(
        "a",
        &ep("ngsi-ld/v1/"),
        json!({ "conformsTo": ["https://smart-data-models.github.io/dataModel.Environment/"] }),
    );
    let schema = dist("b", &ep("schema/model.schema.json"), json!({}));
    let ds = with_dists(dataset("m", json!({})), vec![schema.clone(), sdm]);
    assert_eq!(
        jc_model(&ds, "en", "sk"),
        Some((
            "https://smart-data-models.github.io/dataModel.Environment/".into(),
            "Smart Data Model".into()
        ))
    );
    let ds = with_dists(dataset("m", json!({})), vec![schema]);
    assert_eq!(
        jc_model(&ds, "en", "sk"),
        Some((ep("schema/model.schema.json"), "JSON Schema".into()))
    );
    assert_eq!(jc_model(&dataset("m", json!({})), "en", "sk"), None);
}

#[test]
fn jc_model_never_links_a_javascript_schema_address() {
    let bad = dist(
        "b",
        "javascript:alert(1)//x/schema/model.schema.json",
        json!({}),
    );
    let ds = with_dists(dataset("m", json!({})), vec![bad]);
    assert_eq!(jc_model(&ds, "en", "sk"), None);
}

// ---- jc_resource_sections ----

#[test]
fn jc_resource_sections_group_and_order_like_plugin() {
    let ds = with_dists(
        dataset("air", json!({})),
        vec![
            dist("ctx", &ep("schema/context.jsonld"), json!({})),
            dist("json", &ep("file.json"), json!({})),
            dist("ngsi", &ep("ngsi-ld/v1/"), json!({})),
            dist("csv", &ep("file.csv"), json!({})),
            dist(
                "src",
                "https://city.example/page",
                json!({ "title": { "sk": "Dataset air", "en": "Dataset air en" } }),
            ),
            dist(
                "other",
                "https://city.example/x.zip",
                json!({ "format": "zip" }),
            ),
        ],
    );
    let secs = jc_resource_sections(&ds, "en", "sk", "/en");
    let keys: Vec<&str> = secs.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, ["data", "api", "schema", "source"]);
    let data: Vec<&str> = secs[0].items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(data, ["csv", "json", "other"]);
    assert_eq!(secs[0].title, jc_t("section_data", "en"));
    assert_eq!(secs[0].lead, jc_t("section_data_lead", "en"));
    assert_eq!(secs[0].items[0].action_label, jc_t("download", "en"));
    assert_eq!(secs[0].items[0].page_url, "/en/dataset/air/resource/csv");
    assert_eq!(secs[0].items[0].format_lower, "csv");
    assert!(secs[1].items[0].is_api);
    assert_eq!(secs[1].items[0].action_label, jc_t("open", "en"));
    assert_eq!(secs[3].items[0].action_label, jc_t("open", "en"));
    assert!(jc_resource_sections(&dataset("e", json!({})), "en", "sk", "").is_empty());
}

#[test]
fn jc_resource_sections_never_link_a_non_http_address() {
    let ds = with_dists(
        dataset("x", json!({})),
        vec![
            dist(
                "bad",
                "javascript:alert(document.cookie)",
                json!({ "format": "CSV" }),
            ),
            dist(
                "dl",
                "https://example.org/a.csv",
                json!({ "format": "CSV", "downloadUrl": "data:text/html,<script>x</script>" }),
            ),
            dist(
                "ok",
                "https://example.org/b.csv",
                json!({ "format": "CSV" }),
            ),
        ],
    );
    let items = &jc_resource_sections(&ds, "en", "sk", "")[0].items;
    let url = |id: &str| items.iter().find(|i| i.id == id).expect(id).url.clone();
    assert_eq!(url("bad"), "");
    assert_eq!(url("dl"), "");
    assert_eq!(url("ok"), "https://example.org/b.csv");
}

// ---- jc_about, parse of the `source` extra, jc_frequency, jc_more_rows ----

#[test]
fn jc_about_minimal_record_lists_publisher_and_dates() {
    let rows = jc_about(&dataset("x", json!({})), None, "sk", "sk");
    let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            jc_t("publisher", "sk"),
            jc_t("created", "sk"),
            jc_t("last_update", "sk")
        ]
    );
    assert_eq!(rows[0].text, "city");
    assert_eq!(rows[0].link, None);
    assert_eq!(rows[1].text, "2. januára 2026");
    let none = jc_about(
        &dataset("x", json!({ "publisherId": "" })),
        None,
        "en",
        "sk",
    );
    assert_eq!(none.len(), 2, "no publisher row without a name");
}

#[test]
fn jc_about_full_record_matches_plugin_rows_and_links() {
    let p = publisher("city", json!({ "uri": "javascript:alert(1)" }));
    let ds = dataset(
        "full",
        json!({
            "licenceId": "cc-by",
            "tags": ["air", "noise"],
            "frequency": "http://publications.europa.eu/resource/authority/frequency/HOURLY",
            "spatial": ["SK032"],
            "temporal": { "start": "2020-01-01", "end": "2025-12-31" },
            "contactPoint": { "name": { "en": "Data desk" }, "email": "data@city.example" },
            "identifier": "urn:x:1",
            "conformsTo": ["urn:not-a-link"],
            "extras": {
                "rights": "© City",
                "source": "[{\"url\":\"https://a.example/1\",\"title\":\"First\"},{\"url\":\"https://a.example/2\"},\"https://a.example/3\",\"javascript:alert(1)\",{\"url\":\"ftp://x\"},7]"
            }
        }),
    );
    let rows = jc_about(&ds, Some(&p), "en", "sk");
    let get = |label: &str| -> Vec<&AboutRow> {
        rows.iter()
            .filter(|r| r.label == jc_t(label, "en"))
            .collect()
    };
    assert_eq!(get("publisher")[0].text, "Publisher city");
    assert_eq!(
        get("publisher")[0].link,
        None,
        "a javascript: uri is no link"
    );
    assert_eq!(get("licence")[0].text, "Creative Commons Attribution 4.0");
    assert_eq!(
        get("licence")[0].link.as_deref(),
        Some("https://creativecommons.org/licenses/by/4.0/")
    );
    assert_eq!(get("attribution")[0].text, "© City");
    let sources: Vec<(String, Option<String>)> = get("source")
        .iter()
        .map(|r| (r.text.clone(), r.link.clone()))
        .collect();
    assert_eq!(
        sources,
        vec![
            ("First".into(), Some("https://a.example/1".into())),
            (
                "https://a.example/2".into(),
                Some("https://a.example/2".into())
            ),
            (
                "https://a.example/3".into(),
                Some("https://a.example/3".into())
            ),
        ]
    );
    assert_eq!(get("keywords")[0].text, "air, noise");
    assert_eq!(get("frequency")[0].text, "Every hour");
    assert_eq!(get("next_update")[0].text, "November 5, 2026, 11:30 (UTC)");
    assert_eq!(
        get("coverage")[0].link.as_deref(),
        Some("http://data.europa.eu/nuts/code/SK032")
    );
    assert!(
        get("coverage")[0].text.ends_with("(NUTS SK032)"),
        "{}",
        get("coverage")[0].text
    );
    assert_eq!(get("period")[0].text, "2020-01-01 – 2025-12-31");
    assert_eq!(get("contact")[0].text, "Data desk");
    assert_eq!(
        get("contact")[0].link.as_deref(),
        Some("mailto:data@city.example")
    );
    assert_eq!(get("identifier")[0].text, "urn:x:1");
    assert_eq!(get("conforms_to")[0].link, None);
}

#[test]
fn jc_about_edge_values() {
    // Extras override the publisher; a contact without '@' is text only; mailto: is kept.
    let ds = dataset(
        "e",
        json!({
            "contactPoint": { "name": {}, "email": "mailto:a@b.example" },
            "temporal": { "end": "2025-01-01" },
            "extras": {
                "publisher_name": "From extras", "publisher_uri": "https://pub.example/",
                "source": "not json [", "spatial_uri": "https://example.org/area",
                "identifier": "", "conforms_to": "https://www.w3.org/TR/vocab-dcat-3/"
            }
        }),
    );
    let rows = jc_about(&ds, None, "de", "sk");
    let row = |k: &str| rows.iter().find(|r| r.label == jc_t(k, "de")).cloned();
    assert_eq!(
        row("publisher").expect("publisher").link.as_deref(),
        Some("https://pub.example/")
    );
    assert_eq!(row("publisher").expect("publisher").text, "From extras");
    assert!(
        row("source").is_none(),
        "a source that is no address is not listed"
    );
    assert_eq!(
        row("contact").expect("contact").link.as_deref(),
        Some("mailto:a@b.example")
    );
    assert_eq!(
        row("coverage").expect("coverage").text,
        "https://example.org/area"
    );
    assert_eq!(row("period").expect("period").text, "2025-01-01");
    assert!(row("identifier").is_none());
    assert_eq!(row("conforms_to").expect("standard").text, "DCAT");
    let ds = dataset(
        "e",
        json!({ "contactPoint": { "name": {"en": "Desk"}, "email": "nobody" }, "extras": {"source": "[\"bad"} }),
    );
    let rows = jc_about(&ds, None, "en", "sk");
    let contact = rows
        .iter()
        .find(|r| r.label == jc_t("contact", "en"))
        .expect("contact");
    assert_eq!(contact.link, None);
    let ds = dataset(
        "e",
        json!({ "temporal": {}, "contactPoint": { "name": {}, "email": "" } }),
    );
    let rows = jc_about(&ds, None, "en", "sk");
    assert!(rows
        .iter()
        .all(|r| r.label != jc_t("period", "en") && r.label != jc_t("contact", "en")));
    // One plain address in `source`.
    let ds = dataset(
        "e",
        json!({ "extras": { "source": "https://only.example/" } }),
    );
    assert_eq!(
        jc_about(&ds, None, "en", "sk")
            .iter()
            .filter(|r| r.label == jc_t("source", "en"))
            .count(),
        1
    );
}

#[test]
fn jc_frequency_reads_codes_and_uris_in_every_language() {
    let f =
        |code: &str, lang: &str| jc_frequency(&dataset("f", json!({ "frequency": code })), lang);
    assert!(jc_frequency(&dataset("f", json!({})), "en").is_none());
    let table = [
        (
            "CONT",
            [
                "Priebežne",
                "Průběžně",
                "Continuously",
                "Folyamatosan",
                "Laufend",
            ],
            None,
        ),
        (
            "UPDATE_CONT",
            [
                "Priebežne",
                "Průběžně",
                "Continuously",
                "Folyamatosan",
                "Laufend",
            ],
            None,
        ),
        (
            "HOURLY",
            [
                "Každú hodinu",
                "Každou hodinu",
                "Every hour",
                "Óránként",
                "Stündlich",
            ],
            Some((1, true)),
        ),
        (
            "DAILY",
            ["Denne", "Denně", "Every day", "Naponta", "Täglich"],
            Some((24, false)),
        ),
        (
            "WEEKLY",
            ["Týždenne", "Týdně", "Every week", "Hetente", "Wöchentlich"],
            Some((168, false)),
        ),
        (
            "MONTHLY",
            ["Mesačne", "Měsíčně", "Every month", "Havonta", "Monatlich"],
            Some((720, false)),
        ),
        (
            "QUARTERLY",
            [
                "Štvrťročne",
                "Čtvrtletně",
                "Every quarter",
                "Negyedévente",
                "Vierteljährlich",
            ],
            Some((2184, false)),
        ),
        (
            "ANNUAL",
            ["Ročne", "Ročně", "Every year", "Évente", "Jährlich"],
            Some((8760, false)),
        ),
        (
            "IRREG",
            [
                "Nepravidelne",
                "Nepravidelně",
                "Irregularly",
                "Rendszertelenül",
                "Unregelmäßig",
            ],
            None,
        ),
    ];
    let modified = dataset("f", json!({})).modified;
    for (code, words, next) in table {
        for (lang, word) in LANGS.iter().zip(words) {
            let (label, n) = f(
                &format!("http://publications.europa.eu/resource/authority/frequency/{code}/"),
                lang,
            )
            .expect(code);
            assert_eq!(label, word, "{code} {lang}");
            assert_eq!(
                n.map(|(dt, hourly)| ((dt - modified).num_hours(), hourly)),
                next,
                "{code}"
            );
        }
    }
    assert_eq!(
        f("daily", "fi").expect("daily").0,
        "Every day",
        "unknown language reads English"
    );
    assert_eq!(
        f("every full moon", "sk"),
        Some(("every full moon".into(), None))
    );
    let from_extra = dataset("f", json!({ "extras": { "frequency": "WEEKLY" } }));
    assert_eq!(
        jc_frequency(&from_extra, "en").expect("weekly").0,
        "Every week"
    );
}

#[test]
fn jc_more_rows_hides_known_extras_and_formats_the_quality_rate() {
    let ds = dataset(
        "m",
        json!({ "extras": {
            "quality_pass_rate": "0.873", "source_uri": "https://orig.example/1",
            "was_generated_by": "javascript:alert(1)", "custom_key": "", "some_key": "v",
            "ölevel": "x", "endpoint": "https://hidden", "publisher_name": "hidden"
        }}),
    );
    for lang in LANGS {
        let rows = jc_more_rows(&ds, lang);
        assert_eq!(rows.len(), 5, "{lang}: {rows:?}");
    }
    let rows = jc_more_rows(&ds, "en");
    let by = |t: &str| {
        rows.iter()
            .find(|r| r.label == t)
            .cloned()
            .unwrap_or_else(|| panic!("{t}: {rows:?}"))
    };
    assert_eq!(by("Passed the quality checks").text, "87 %");
    assert_eq!(
        by("Original record").link.as_deref(),
        Some("https://orig.example/1")
    );
    assert_eq!(by("Produced by").link, None, "javascript: is text");
    assert_eq!(by("Some key").text, "v");
    assert_eq!(by("ölevel").text, "x", "a non-ASCII first letter is kept");
    let sk = jc_more_rows(&ds, "sk");
    assert!(sk.iter().any(|r| r.label == "Prešlo kontrolami kvality"));
    for (raw, shown) in [("1.5", "1.5"), ("abc", "abc"), ("0", "0 %"), ("1", "100 %")] {
        let ds = dataset("q", json!({ "extras": { "quality_pass_rate": raw } }));
        assert_eq!(jc_more_rows(&ds, "en")[0].text, shown, "{raw}");
    }
    let labels: Vec<String> = ["cs", "de", "hu"]
        .iter()
        .flat_map(|l| jc_more_rows(&ds, l).into_iter().map(|r| r.label))
        .collect();
    assert!(
        labels.iter().all(|l| l != "Passed the quality checks"),
        "{labels:?}"
    );
}

// ---- jc_subscriptions, jc_visualizations, schema.org ----

#[test]
fn jc_subscriptions_offer_calendar_and_feeds_from_https_addresses() {
    let ds = with_dists(
        dataset("s", json!({})),
        vec![
            dist(
                "ics",
                "https://x.example/cal.ics",
                json!({ "mediaType": "text/calendar; charset=utf-8" }),
            ),
            dist("ics2", "https://x.example/c2", json!({ "format": "ics" })),
            dist(
                "atom",
                "https://x.example/feed.atom",
                json!({ "mediaType": "application/atom+xml" }),
            ),
            dist(
                "rss",
                "https://x.example/feed.rss",
                json!({ "mediaType": "application/rss+xml" }),
            ),
            dist(
                "jf",
                "https://x.example/feed.json",
                json!({ "mediaType": "application/feed+json" }),
            ),
            dist(
                "http",
                "http://x.example/cal.ics",
                json!({ "mediaType": "text/calendar" }),
            ),
            dist(
                "csv",
                "https://x.example/a.csv",
                json!({ "mediaType": "text/csv" }),
            ),
        ],
    );
    let s = jc_subscriptions(&ds, "en", "sk");
    assert!(!s.is_empty());
    assert_eq!(s.calendar.len(), 2);
    assert_eq!(s.calendar[0].2, "webcal://x.example/cal.ics");
    assert_eq!(s.calendar[0].3, "text/calendar");
    assert_eq!(s.calendar[0].0, "Resource ics");
    let mimes: Vec<&str> = s.news.iter().map(|n| n.2.as_str()).collect();
    assert_eq!(
        mimes,
        [
            "application/atom+xml",
            "application/rss+xml",
            "application/feed+json"
        ]
    );
    assert!(jc_subscriptions(&dataset("e", json!({})), "en", "sk").is_empty());
}

#[test]
fn jc_visualizations_frame_charts_on_viz_with_the_title_escaped() {
    let ds = dataset("v", json!({}));
    let views = [
        view(
            "c1",
            "jc_visualization",
            "Air <script>alert(1)</script> \"q\"",
        ),
        view("t1", "datatables_view", "Table"),
    ];
    assert!(jc_visualizations(&ds, &views, &BrandingBlock::default(), "sk", "").is_empty());
    let charts = jc_visualizations(&ds, &views, &branding(), "cs-CZ", "/cs");
    assert_eq!(charts.len(), 1);
    let c = &charts[0];
    assert_eq!(c.src, format!("https://viz.{DOMAIN}/#c1&inline&lang=cs"));
    assert_eq!(c.origin, format!("https://viz.{DOMAIN}"));
    assert_eq!(c.package_link, "/cs/dataset/v#jc-preview-title");
    assert!(c
        .code
        .contains("title=\"Air &lt;script&gt;alert(1)&lt;/script&gt; &quot;q&quot;\""));
    assert!(c
        .code
        .contains(&format!("src=\"https://viz.{DOMAIN}/#c1&lang=cs\"")));
    assert!(c.code.contains("sandbox=\"allow-scripts"));
}

#[test]
fn jc_schema_org_script_cannot_close_its_script_element() {
    let c = cat();
    let ds = dataset(
        "s",
        json!({ "title": { "sk": "</script><script>alert(1)</script> & co" }, "description": {} }),
    );
    let script = jc_schema_org_script(&c, &ds, None, &branding(), "sk");
    assert!(
        !script.contains('<') && !script.contains('>') && !script.contains('&'),
        "{script}"
    );
    let back: Value = serde_json::from_str(&script).expect("JSON");
    assert_eq!(back["name"], "</script><script>alert(1)</script> & co");
    assert_eq!(
        back["description"], back["name"],
        "description falls back to the title"
    );
}

#[test]
fn jc_schema_org_dict_keeps_only_http_links_and_dataset_values() {
    let c = cat();
    let p = publisher("city", json!({ "uri": "urn:not-http" }));
    let long = "é".repeat(6000);
    let ds = with_dists(
        dataset(
            "s",
            json!({
                "description": { "sk": long },
                "licenceUrl": "javascript:alert(1)",
                "spatial": ["SK0"],
                "temporal": { "start": "2021-03-04" },
                "tags": ["a"],
                "identifier": "ident-1"
            }),
        ),
        vec![
            dist("a", "https://x.example/a.csv", json!({ "format": "CSV" })),
            dist("b", "https://x.example/b", json!({})),
            dist("c", "javascript:x", json!({ "format": "CSV" })),
            dist(
                "d",
                "https://x.example/d",
                json!({ "mediaType": "text/csv", "title": {} }),
            ),
        ],
    );
    let v = jc_schema_org_dict(&c, &ds, Some(&p), &branding(), "sk");
    assert_eq!(v["@type"], "Dataset");
    assert_eq!(v["url"], format!("https://{HOST}/dataset/s"));
    assert_eq!(
        v["description"].as_str().expect("desc").chars().count(),
        5000
    );
    assert_eq!(v["creator"]["name"], "Vydavateľ city");
    assert!(v["creator"].get("sameAs").is_none());
    assert!(v.get("license").is_none());
    assert_eq!(
        v["spatialCoverage"]["sameAs"],
        "http://data.europa.eu/nuts/code/SK0"
    );
    assert_eq!(v["temporalCoverage"], "2021-03-04/..");
    assert_eq!(v["keywords"], json!(["a"]));
    assert_eq!(v["identifier"], "ident-1");
    let dists = v["distribution"].as_array().expect("distributions");
    assert_eq!(dists.len(), 2);
    assert_eq!(dists[0]["name"], "Zdroj a");
    assert_eq!(
        dists[1]["name"], "https://x.example/d",
        "no title, no format: the address"
    );
    assert_eq!(dists[1]["encodingFormat"], "text/csv");

    let bare = dataset(
        "b",
        json!({ "licenceUrl": "https://lic.example/", "extras": { "publisher_uri": "https://pub.example/" } }),
    );
    let v = jc_schema_org_dict(&c, &bare, None, &branding(), "en");
    assert_eq!(v["license"], "https://lic.example/");
    assert_eq!(v["creator"]["sameAs"], "https://pub.example/");
    assert_eq!(v["creator"]["name"], "city");
    assert_eq!(v["temporalCoverage"], "2026-01-02/..");
    assert_eq!(v["identifier"], "id-b");
    assert!(
        v.get("keywords").is_none()
            && v.get("distribution").is_none()
            && v.get("spatialCoverage").is_none()
    );
}

// ---- jc_mcp_install, url_parts ----

#[test]
fn jc_mcp_install_builds_every_client_snippet_from_one_address() {
    let mut b = branding();
    b.apply_override_json(&json!({ "mcpServer": "d/air-quality" }));
    let m = jc_mcp_install(&b).expect("mcp");
    let url = format!("https://api.{DOMAIN}/api/mcp/d/air-quality");
    assert_eq!(m.url, url);
    assert_eq!(
        m.claude_copy,
        format!("claude mcp add --transport http joinedcontext {url}")
    );
    let json_copy: Value = serde_json::from_str(&m.json_copy).expect("json");
    assert_eq!(json_copy["mcpServers"]["joinedcontext"]["url"], url);
    let enc = m
        .vscode
        .strip_prefix("vscode:mcp/install?")
        .expect("vscode prefix");
    let decoded: String = url::form_urlencoded::parse(format!("x={enc}").as_bytes())
        .next()
        .expect("pair")
        .1
        .into_owned();
    let payload: Value = serde_json::from_str(&decoded).expect("vscode payload");
    assert_eq!(payload["url"], url);
    assert!(m
        .cursor
        .starts_with("cursor://anysphere.cursor-deeplink/mcp/install?name=joinedcontext&config="));
    assert!(m.claude_breakable.contains("jc-nb") && m.json_breakable.contains("jc-nb"));
    // An mcpServer that is not `d/{id}` is ignored.
    let mut odd = branding();
    odd.mcp_server = Some("../../admin".into());
    assert_eq!(
        jc_mcp_install(&odd).expect("mcp").url,
        format!("https://api.{DOMAIN}/api/mcp")
    );
}

#[test]
fn url_parts_edge_cases() {
    assert_eq!(url_parts(""), vec![""]);
    assert_eq!(url_parts("/"), vec!["/"]);
    assert_eq!(url_parts("a//b/c"), vec!["a//b", "/c"]);
    assert_eq!(url_parts("ab"), vec!["ab"]);
}

// ---- jc_page, jc_try_prompt, jc_assistant_guide ----

#[test]
fn jc_page_serves_five_pages_in_slovak_and_english() {
    let b = branding();
    assert!(jc_page("admin", &b, "en").is_none());
    assert!(jc_page("", &b, "en").is_none());
    for name in STATIC_PAGE_NAMES {
        let sk = jc_page(name, &b, "sk").expect(name);
        let en = jc_page(name, &b, "en").expect(name);
        let de = jc_page(name, &b, "de").expect(name);
        assert_ne!(sk.title, en.title, "{name}");
        assert_eq!(de.title, en.title, "{name}: other languages read English");
        assert!(!sk.sections.is_empty());
        assert_eq!(en.other_pages.len(), 4);
        assert!(en.other_pages.iter().all(|(p, _)| p != name));
        assert_eq!(en.email, "");
    }
    let connector = jc_page("connector", &b, "en").expect("connector");
    assert!(connector.sections[0].1[0].contains(&format!("https://api.{DOMAIN}/api/mcp")));
    let terms = jc_page("terms", &b, "sk").expect("terms");
    assert!(terms
        .sections
        .iter()
        .flat_map(|s| &s.1)
        .any(|p| p.contains(DOMAIN)));
    let mut with_mail = branding();
    with_mail.apply_override_json(&json!({ "contactEmail": "data@city.example" }));
    assert_eq!(
        jc_page("support", &with_mail, "en").expect("support").email,
        "data@city.example"
    );
}

#[test]
fn jc_try_prompt_uses_the_branding_sample_then_the_default_question() {
    let mut b = branding();
    b.apply_override_json(&json!({ "samples": { "sk": ["Aké je ovzdušie?"] } }));
    let sk = jc_try_prompt(&b, "sk-SK");
    assert!(sk.ends_with("Aké je ovzdušie?"), "{sk}");
    assert!(sk.contains(CONNECTOR_NAME));
    let en = jc_try_prompt(&b, "en");
    assert!(en.ends_with(&jc_t("guide_example", "en")), "{en}");
}

#[test]
fn jc_assistant_guide_has_three_guides_in_two_languages() {
    let b = branding();
    assert!(jc_assistant_guide("gemini", &b, "en").is_none());
    for (id, name, paste) in [
        ("claude", "Claude", 3),
        ("chatgpt", "ChatGPT", 3),
        ("grok", "Grok", 2),
    ] {
        let sk = jc_assistant_guide(id, &b, "sk").expect(id);
        let en = jc_assistant_guide(id, &b, "en").expect(id);
        assert_eq!(en.name, name);
        assert_eq!(en.paste_index, paste);
        assert!(en.steps.len() > paste);
        assert_ne!(sk.steps, en.steps, "{id}");
        assert!(en.help.starts_with("https://"));
        assert!(en.title.contains(name));
        assert!(en.checked.contains("2026-10-18") && en.checked.contains(name));
        assert!(en.good.contains(CONNECTOR_NAME));
        assert_eq!(en.prompt, en.example);
        assert!(!en.trouble.is_empty() && !en.who.is_empty());
    }
}

// ---- sitemap, robots, publisher card ----

#[test]
fn jc_sitemap_xml_lists_public_records_escaped_and_sorted() {
    let mut c = cat();
    c.hidden_datasets.insert("hidden".into(), "test".into());
    let dss = [
        dataset("zeta", json!({})),
        dataset("a&b", json!({})),
        dataset("internal", json!({ "visibility": "internal" })),
        dataset("gone", json!({ "state": "withdrawn" })),
        dataset("hidden", json!({})),
        dataset("orphaned", json!({ "publisherId": "old" })),
        dataset("by-id", json!({ "publisherId": "pub-old" })),
        dataset("unlisted-publisher", json!({ "publisherId": "elsewhere" })),
    ];
    let pubs = [
        publisher("city", json!({})),
        publisher("old", json!({ "state": "withdrawn" })),
    ];
    let xml = jc_sitemap_xml(&c, &dss, &pubs);
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset"));
    assert!(xml.ends_with("</urlset>\n"));
    let a = xml
        .find(&format!("https://{HOST}/dataset/a&amp;b"))
        .expect("escaped name");
    let z = xml.find("/dataset/zeta").expect("zeta");
    assert!(a < z, "sorted by name");
    assert!(xml.contains("<lastmod>2026-11-05</lastmod>"));
    assert!(xml.contains("/organization/city</loc>"));
    // A dataset whose publisher record is absent stays listed, as package_show shows it.
    assert!(xml.contains("/dataset/unlisted-publisher</loc>"));
    for absent in [
        "internal",
        "gone",
        "hidden",
        "/organization/old",
        "orphaned",
        "by-id",
    ] {
        assert!(!xml.contains(absent), "{absent}");
    }
    let empty = jc_sitemap_xml(&c, &[], &[]);
    assert_eq!(empty.lines().count(), 3);
}

#[test]
fn jc_robots_txt_disallows_all_when_noindex() {
    let c = cat();
    let mut b = branding();
    let open = jc_robots_txt(&c, &b);
    assert!(open.contains(&format!("Sitemap: https://{HOST}/sitemap.xml")));
    assert!(open.contains("Disallow: /api/\n") && !open.contains("Disallow: /\n"));
    b.apply_override_json(&json!({ "noindex": true }));
    assert_eq!(jc_robots_txt(&c, &b), "User-agent: *\nDisallow: /\n");
}

#[test]
fn publisher_card_info_falls_back_to_name_badge_and_placeholder() {
    let p = publisher(
        "bratislava-city",
        json!({ "title": {}, "description": { "sk": "Mesto" } }),
    );
    let (title, image, desc) = publisher_card_info(&p, "en", "sk");
    assert_eq!(title, "bratislava-city");
    assert!(!image.is_empty());
    assert_eq!(desc.as_deref(), Some("Mesto"));
    let logo = publisher("x", json!({ "logoUrl": "https://x.example/logo.png" }));
    assert_eq!(
        publisher_card_info(&logo, "sk", "sk").1,
        "https://x.example/logo.png"
    );
    assert_eq!(publisher_card_info(&logo, "sk", "sk").0, "Vydavateľ x");
}

// ---- small text helpers ----

#[test]
fn nuts_label_names_codes_and_leaves_other_text() {
    assert!(nuts_label("SK032", "sk").ends_with("(NUTS SK032)"));
    assert_ne!(nuts_label("SK032", "sk"), "NUTS SK032");
    assert_eq!(
        nuts_label("http://data.europa.eu/nuts/code/SK032/", "en"),
        nuts_label("SK032", "en")
    );
    assert_eq!(nuts_label("ZZ999", "en"), "NUTS ZZ999");
    for other in [
        "sk032",
        "S",
        "",
        "Žilina",
        "SK0322X",
        "https://example.org/area",
    ] {
        assert_eq!(nuts_label(other, "en"), other, "{other:?}");
    }
    // An unknown language reads the English name.
    assert_eq!(nuts_label("SK032", "fi"), nuts_label("SK032", "en"));
}

#[test]
fn jc_plain_glyphs_maps_symbol_font_bullets_and_drops_other_private_use() {
    assert_eq!(
        jc_plain_glyphs("\u{f0b7}a\u{f0a7}b\u{f0a8}c\u{f0d8}d\u{f0fc}e\u{f076}f\u{e000}g\u{f8ff}"),
        "•a▪b◦c›d✓e❖fg"
    );
    assert_eq!(jc_plain_glyphs(""), "");
    assert_eq!(jc_plain_glyphs("Žilina <b>"), "Žilina <b>");
}

#[test]
fn jc_pieces_breaks_only_after_slash_query_and_ampersand() {
    let texts: Vec<(String, bool)> = jc_pieces("  curl https://a//b/c?d=1&e=2 x ")
        .into_iter()
        .map(|p| (p.text, p.space))
        .collect();
    assert_eq!(
        texts,
        vec![
            ("  ".into(), true),
            ("curl".into(), false),
            (" ".into(), true),
            ("https://".into(), false),
            ("a//".into(), false),
            ("b/".into(), false),
            ("c?".into(), false),
            ("d=1&".into(), false),
            ("e=2".into(), false),
            (" ".into(), true),
            ("x".into(), false),
            (" ".into(), true),
        ]
    );
    assert!(jc_pieces("").is_empty());
    assert_eq!(
        render_breakable_html("a<b> c"),
        "<span class=\"jc-nb\">a&lt;b&gt;</span><wbr> <span class=\"jc-nb\">c</span><wbr>"
    );
    assert_eq!(html_escape("&<>\"'"), "&amp;&lt;&gt;&quot;&#39;");
}

#[test]
fn jc_kind_reads_the_endpoint_path() {
    let k = |u: &str| jc_kind(&dist("r", u, json!({})));
    for (path, kind) in [
        ("", "DCAT"),
        ("ngsi-ld/v1/entities", "NGSI-LD"),
        ("ngsi-ld", "NGSI-LD"),
        ("odata/v4/A", "ODATA"),
        ("ogc/features/", "OGCFEAT"),
        ("sta/v1.1/Things", "STA"),
        ("schema/model.md", "MODEL"),
        ("mcp", "MCP"),
        ("ws", "WEBSOCKET"),
        ("file.csv", "CSV"),
        ("file.json", "JSON"),
        ("file.geojson", "GEOJSON"),
        ("file.xlsx", "XLSX"),
        ("file.parquet", "PARQUET"),
    ] {
        assert_eq!(k(&ep(path)), Some(kind), "{path}");
    }
    assert_eq!(k(&ep("ws").replace("https", "wss")), Some("WEBSOCKET"));
    for none in [
        ep("file.txt"),
        "javascript:alert(1)".into(),
        "ftp://h/api/endpoint/s/file.csv".into(),
        "https://h.example/other/s/file.csv".into(),
        "https://h.example/api/endpoint/slugonly".into(),
        "not a url".into(),
    ] {
        assert_eq!(k(&none), None, "{none}");
    }
}

#[test]
fn format_chips_and_labels_follow_data_formats_order() {
    assert_eq!(data_format_label(" csv "), "CSV");
    assert_eq!(data_format_label("ogcfeat"), "OGC API Features");
    assert_eq!(data_format_label("Shapefile"), "Shapefile");
    let ds = with_dists(
        dataset("f", json!({})),
        vec![
            dist("a", &ep("file.csv"), json!({})),
            dist(
                "b",
                "https://x.example/c.ics",
                json!({ "mediaType": "text/calendar" }),
            ),
            dist(
                "c",
                "https://x.example/feed",
                json!({ "mediaType": "application/atom+xml" }),
            ),
            dist(
                "d",
                "https://x.example/rss",
                json!({ "mediaType": "application/rss+xml" }),
            ),
            dist(
                "e",
                "https://x.example/jf",
                json!({ "mediaType": "application/feed+json" }),
            ),
            dist("f", "https://x.example/x.xml", json!({ "format": " xml " })),
            dist("g", &ep("ngsi-ld/v1/"), json!({})),
            dist("h", &ep("schema/model.schema.json"), json!({})),
            dist(
                "i",
                "https://x.example/page",
                json!({ "title": { "sk": "Dataset f" } }),
            ),
            dist("j", "https://x.example/odd", json!({ "format": "SHP" })),
        ],
    );
    assert_eq!(
        jc_data_formats(&ds),
        ["NGSI-LD", "CSV", "XML", "ICS", "RSS", "ATOM", "JSONFEED"]
    );
    assert_eq!(
        jc_format_chips(&ds),
        [
            "NGSI-LD",
            "CSV",
            "XML",
            "iCalendar",
            "RSS",
            "Atom",
            "JSON Feed"
        ]
    );
    assert!(jc_format_chips(&dataset("e", json!({}))).is_empty());
}

#[test]
fn resource_keys_chips_and_labels_per_kind() {
    let ds = dataset("air", json!({}));
    let mut sorted = [
        dist("z", &ep("mcp"), json!({})),
        dist("y", &ep("file.json"), json!({})),
        dist("x", "https://x.example/x.zip", json!({ "format": "zip" })),
        dist("w", &ep("file.csv"), json!({})),
        dist("v", "https://x.example/v", json!({ "format": "dcat" })),
    ];
    sorted.sort_by_key(resource_sort_key);
    let ids: Vec<&str> = sorted.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, ["w", "y", "x", "z", "v"]);
    assert_eq!(
        resource_section_key(&sorted[4], &ds),
        "api",
        "format DCAT is an API"
    );
    assert_eq!(resource_section_key(&sorted[2], &ds), "data");
    for (path, chip) in [
        ("", "DCAT-AP"),
        ("ngsi-ld/v1/", "NGSI-LD"),
        ("ws", "WebSocket"),
        ("mcp", "MCP"),
        ("ogc/x", "OGC API"),
        ("odata/x", "OData"),
        ("sta/x", "SensorThings"),
        ("file.csv", "CSV"),
        ("file.json", "JSON"),
        ("file.geojson", "GeoJSON"),
        ("file.xlsx", "Excel"),
        ("file.parquet", "Parquet"),
    ] {
        assert_eq!(
            jc_resource_format_chip(&dist("r", &ep(path), json!({}))),
            chip
        );
    }
    assert_eq!(
        jc_resource_format_chip(&dist("r", "https://x/a", json!({ "format": " PDF " }))),
        "PDF"
    );
    assert_eq!(
        jc_resource_format_chip(&dist("r", "https://x/a", json!({ "format": " " }))),
        "—"
    );
    assert_eq!(
        jc_resource_format_chip(&dist("r", "https://x/a", json!({}))),
        "—"
    );
}

#[test]
fn jc_resource_label_names_model_files_kinds_and_falls_back() {
    let ds = dataset("air", json!({}));
    let files = [
        (
            "context.jsonld",
            [
                "Kontext JSON-LD",
                "Kontext JSON-LD",
                "JSON-LD context",
                "JSON-LD kontextus",
                "JSON-LD-Kontext",
            ],
        ),
        ("model.linkml.yaml", ["LinkML"; 5]),
        (
            "model.md",
            [
                "Popis slovami",
                "Popis slovy",
                "Described in words",
                "Szöveges leírás",
                "In Worten beschrieben",
            ],
        ),
        ("model.owl.ttl", ["OWL"; 5]),
        ("model.rdf.ttl", ["RDF"; 5]),
        ("model.schema.json", ["JSON Schema"; 5]),
        (
            "index.json",
            [
                "Zoznam súborov",
                "Seznam souborů",
                "List of its files",
                "A fájlok listája",
                "Liste der Dateien",
            ],
        ),
        (
            "model.shacl.ttl",
            [
                "Pravidlá kontroly",
                "Pravidla kontroly",
                "Validation rules",
                "Ellenőrzési szabályok",
                "Prüfregeln",
            ],
        ),
    ];
    for (file, words) in files {
        let d = dist("m", &ep(&format!("schema/{file}?v=1")), json!({}));
        for (lang, word) in LANGS.iter().zip(words) {
            assert_eq!(
                jc_resource_label(&d, &ds, lang, "sk"),
                word,
                "{file} {lang}"
            );
        }
    }
    // jcctl names model files "{dataset} {file}" outside a schema path.
    let named = dist(
        "n",
        "https://x.example/f",
        json!({ "title": { "en": "air model.md" } }),
    );
    assert_eq!(
        jc_resource_label(&named, &ds, "en", "sk"),
        "Described in words"
    );
    for (path, key) in [
        ("file.csv", "records_csv"),
        ("file.json", "records_json"),
        ("file.geojson", "records_geojson"),
        ("file.xlsx", "records_xlsx"),
        ("file.parquet", "records_parquet"),
        ("ws", "live_changes"),
    ] {
        assert_eq!(
            jc_resource_label(&dist("r", &ep(path), json!({})), &ds, "cs", "sk"),
            jc_t(key, "cs")
        );
    }
    for (path, label) in [
        ("", "DCAT-AP record"),
        ("ngsi-ld/v1/", "NGSI-LD API"),
        ("mcp", "Model Context Protocol"),
        ("ogc/x", "OGC API - Features"),
        ("odata/x", "OData v4"),
        ("sta/x", "SensorThings API"),
    ] {
        assert_eq!(
            jc_resource_label(&dist("r", &ep(path), json!({})), &ds, "sk", "sk"),
            label
        );
    }
    let source = dist(
        "s",
        "https://city.example/p",
        json!({ "title": { "sk": "Dataset air" } }),
    );
    assert_eq!(
        jc_resource_label(&source, &ds, "sk", "sk"),
        jc_t("at_publisher", "sk")
    );
    let prefixed = dist(
        "p",
        "https://x/a",
        json!({ "title": { "en": "air  Hourly readings" }, "format": "PDF" }),
    );
    assert_eq!(
        jc_resource_label(&prefixed, &ds, "en", "sk"),
        "Hourly readings"
    );
    let only_name = dist(
        "o",
        "https://x/a",
        json!({ "title": { "en": "air " }, "format": "PDF" }),
    );
    assert_eq!(jc_resource_label(&only_name, &ds, "en", "sk"), "air");
    let untitled = dist("u", "https://x/a", json!({ "title": {}, "format": "PDF" }));
    assert_eq!(jc_resource_label(&untitled, &ds, "en", "sk"), "PDF");
    let bare = dist("id-9", "https://x/a", json!({ "title": {} }));
    assert_eq!(jc_resource_label(&bare, &ds, "en", "sk"), "id-9");
}

#[test]
fn jc_what_says_what_each_kind_is_in_every_language() {
    let kinds: [(&str, Value); 15] = [
        ("ngsi-ld/v1/", json!({})),
        ("x.jsonld", json!({ "format": "json-ld" })),
        ("file.geojson", json!({})),
        ("file.csv", json!({})),
        ("file.json", json!({})),
        ("file.xlsx", json!({})),
        ("file.parquet", json!({})),
        ("x.zip", json!({ "format": "zip" })),
        ("ws", json!({})),
        ("ogc/x", json!({})),
        ("odata/x", json!({})),
        ("sta/x", json!({})),
        ("mcp", json!({})),
        ("m.yaml", json!({ "format": "LinkML" })),
        ("m.ttl", json!({ "format": "shacl" })),
    ];
    for (path, patch) in kinds {
        let d = dist("r", &ep(path), patch);
        let en = jc_what(&d, "en");
        assert!(en.ends_with('.'), "{path}: {en}");
        for lang in ["sk", "cs", "hu", "de"] {
            let w = jc_what(&d, lang);
            assert!(w.ends_with('.') && w != en, "{path} {lang}: {w}");
        }
        assert_eq!(jc_what(&d, "fi"), en, "unknown language reads English");
    }
    assert_eq!(
        jc_what(&dist("r", "https://x/a", json!({ "format": "PDF" })), "en"),
        ""
    );
    assert_eq!(jc_what(&dist("r", "https://x/a", json!({})), "en"), "");
}

#[test]
fn jc_entity_type_reads_the_query_the_path_or_nothing() {
    let t = |u: &str| jc_entity_type(&dist("r", u, json!({})));
    assert_eq!(
        t(&ep("ngsi-ld/v1/entities?type=A&q=x")).as_deref(),
        Some("A")
    );
    assert_eq!(t(&ep("ngsi-ld/v1/entities?type=")), None);
    assert_eq!(t(&ep("odata/v4/Bike/x")).as_deref(), Some("Bike"));
    assert_eq!(
        t(&ep("ogc/features/collections/Tree/items")).as_deref(),
        Some("Tree")
    );
    assert_eq!(t(&ep("sta/v1.1/Things")).as_deref(), Some("Things"));
    assert_eq!(t(&ep("odata/v4/")), None);
    assert_eq!(t(&ep("mcp")), None);
    assert_eq!(t("https://x.example/a"), None);
    assert_eq!(t("not a url"), None);
}

#[test]
fn jc_about_publisher_without_a_title_shows_its_name_as_ckan_display_name() {
    // plugin.py: extras publisher_name, else organization title, else organization name.
    let p = publisher("mesto-x", json!({ "title": {} }));
    let rows = jc_about(
        &dataset("t", json!({ "publisherId": "mesto-x" })),
        Some(&p),
        "en",
        "sk",
    );
    let row = rows
        .iter()
        .find(|r| r.label == jc_t("publisher", "en"))
        .expect("publisher row");
    assert_eq!(row.text, "mesto-x");
    assert_eq!(
        row.link.as_deref(),
        Some("https://example.org/publisher/mesto-x")
    );
}

#[test]
fn jc_schema_org_dict_temporal_coverage_with_both_ends() {
    // plugin.py: "%s/%s" % (temporal_start, temporal_end or "..").
    let ds = dataset(
        "t",
        json!({ "temporal": { "start": "2020-01-01", "end": "2025-12-31" } }),
    );
    let v = jc_schema_org_dict(&cat(), &ds, None, &branding(), "sk");
    assert_eq!(v["temporalCoverage"], "2020-01-01/2025-12-31");
}

#[test]
fn publisher_card_info_never_puts_a_bad_colour_into_the_badge() {
    // A colour that is no hex value makes no badge: the card falls back to the CKAN placeholder.
    let p = publisher(
        "x",
        json!({ "badgeColour": "red\"/><script>alert(1)</script>" }),
    );
    let (_, image, _) = publisher_card_info(&p, "sk", "sk");
    assert_eq!(image, "/base/images/placeholder-organization.png");
    let p = publisher("-", json!({}));
    assert_eq!(
        publisher_card_info(&p, "sk", "sk").1,
        "/base/images/placeholder-organization.png",
        "no initials"
    );
}

#[test]
fn jc_is_source_compares_the_titles_in_one_language() {
    // plugin.py: the resource name equals the dataset title. With titles per language the two are
    // compared in the same language, never the first entry of each map.
    let ds = dataset(
        "s",
        json!({ "title": { "cs": "Ovzduší", "sk": "Ovzdušie" } }),
    );
    let same = dist(
        "a",
        "https://city.example/page",
        json!({ "title": { "sk": "Ovzdušie" } }),
    );
    assert!(jc_is_source(&same, &ds), "the sk titles match");
    let ds = dataset("s", json!({ "title": { "en": "Air" } }));
    let other_lang = dist(
        "a",
        "https://city.example/page",
        json!({ "title": { "sk": "Air" } }),
    );
    assert!(
        !jc_is_source(&other_lang, &ds),
        "en title against an sk title"
    );
    let csv = dist(
        "a",
        "https://city.example/a.csv",
        json!({ "title": { "en": "Air" }, "format": "CSV" }),
    );
    assert!(
        !jc_is_source(&csv, &ds),
        "a file with a format is a download"
    );
    let blank = dataset("b", json!({ "title": { "en": " " } }));
    let blank_dist = dist(
        "a",
        "https://city.example/page",
        json!({ "title": { "en": " " } }),
    );
    assert!(
        !jc_is_source(&blank_dist, &blank),
        "an empty title names nothing"
    );
}

#[test]
fn no_domain_never_matches_a_fully_qualified_host_with_a_trailing_dot() {
    // Without a domain the suffix test would be `ends_with(".")`, which every absolute DNS name
    // (`evil.example.`) passes: on_platform refuses an empty domain first.
    let none = BrandingBlock::default();
    assert!(jc_live(&live_ds("https://evil.example./api/"), &none).is_none());
    let d = dist(
        "r",
        "https://evil.example./ngsi-ld/v1/entities?type=Air",
        json!({}),
    );
    assert!(jc_preview(&d, &none, "en").is_none());
}
