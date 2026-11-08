#![allow(dead_code)]
//! Records for the web tests, built from the JSON the store and harvest write.

use dataroom_core::{BrandingBlock, Catalogue, Dataset, Distribution, Publisher, ResourceView};
use serde_json::{json, Value};

pub const DOMAIN: &str = "dev.joinedcontext.com";
pub const SLUG: &str = "esbxdrr7snorjsp7uus2luihr6vl5vv6";
pub const HOST: &str = "opendata.dev.joinedcontext.com";
pub const NONCE: &str = "n0nce-T3st";

/// An address of the test Endpoint: `https://api.{DOMAIN}/api/endpoint/{SLUG}/{path}`.
pub fn ep(path: &str) -> String {
    format!("https://api.{DOMAIN}/api/endpoint/{SLUG}/{path}")
}

pub fn branding() -> BrandingBlock {
    let mut b = BrandingBlock::default();
    b.apply_override_json(&json!({ "domain": DOMAIN }));
    b
}

/// `base` with every member of `patch` set over it.
pub fn merged(mut base: Value, patch: Value) -> Value {
    if let (Some(b), Value::Object(p)) = (base.as_object_mut(), patch) {
        for (k, v) in p {
            b.insert(k, v);
        }
    }
    base
}

pub fn dist(id: &str, url: &str, patch: Value) -> Distribution {
    serde_json::from_value(merged(
        json!({
            "id": id, "datasetId": "ds-1",
            "title": {"sk": format!("Zdroj {id}"), "en": format!("Resource {id}")},
            "accessUrl": url
        }),
        patch,
    ))
    .expect("distribution fixture")
}

pub fn dataset(name: &str, patch: Value) -> Dataset {
    serde_json::from_value(merged(
        json!({
            "orgId": "00000000-0000-0000-0000-000000000000",
            "catalogueId": "00000000-0000-0000-0000-000000000000",
            "id": format!("id-{name}"), "name": name,
            "title": {"sk": format!("Dataset {name}"), "en": format!("Dataset {name} en")},
            "description": {"sk": "Popis dát", "en": "About the data"},
            "publisherId": "city",
            "visibility": "public", "state": "active",
            "issued": "2026-01-02T08:00:00Z", "modified": "2026-11-05T10:30:00Z"
        }),
        patch,
    ))
    .expect("dataset fixture")
}

pub fn with_dists(mut ds: Dataset, dists: Vec<Distribution>) -> Dataset {
    ds.distributions = dists;
    ds
}

pub fn catalogue(branding: Value) -> Catalogue {
    serde_json::from_value(json!({
        "orgId": "00000000-0000-0000-0000-000000000000",
        "id": "00000000-0000-0000-0000-000000000000",
        "name": "dev", "host": HOST,
        "title": {"sk": "Otvorené dáta", "en": "Open Data"},
        "description": {"sk": "Popis"},
        "defaultLanguage": "sk", "languages": ["sk", "en", "cs"],
        "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
        "branding": branding,
        "modified": "2026-11-01T10:00:00Z"
    }))
    .expect("catalogue fixture")
}

/// The test catalogue on [`DOMAIN`].
pub fn cat() -> Catalogue {
    catalogue(json!({ "domain": DOMAIN }))
}

pub fn publisher(name: &str, patch: Value) -> Publisher {
    serde_json::from_value(merged(
        json!({
            "orgId": "00000000-0000-0000-0000-000000000000",
            "catalogueId": "00000000-0000-0000-0000-000000000000",
            "id": format!("pub-{name}"), "name": name,
            "title": {"sk": format!("Vydavateľ {name}"), "en": format!("Publisher {name}")},
            "description": {},
            "uri": format!("https://example.org/publisher/{name}"),
            "state": "active", "editorGrants": [],
            "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
        }),
        patch,
    ))
    .expect("publisher fixture")
}

pub fn view(id: &str, view_type: &str, title: &str) -> ResourceView {
    serde_json::from_value(json!({
        "id": id, "resourceId": "r1", "viewType": view_type, "title": title,
        "description": "Popis grafu"
    }))
    .expect("view fixture")
}

/// Every `<tag ...>` opening of `tag` in `html`.
pub fn tags<'a>(html: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}");
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i..];
        let end = after.find('>').map_or(after.len(), |e| e + 1);
        let next = after.as_bytes().get(open.len()).copied();
        if matches!(next, Some(b' ' | b'>' | b'\n')) {
            out.push(&after[..end]);
        }
        rest = &after[end..];
    }
    out
}

/// Runs `printf '%s\n' <words>` through `/bin/sh` and returns the words the shell parsed: what a
/// reader's shell does with a copied command.
pub fn shell_words(words: &str) -> Vec<String> {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("printf '%s\\n' {words}"))
        .output()
        .expect("run sh");
    assert!(out.status.success(), "sh failed: {out:?}");
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .lines()
        .map(str::to_owned)
        .collect()
}
