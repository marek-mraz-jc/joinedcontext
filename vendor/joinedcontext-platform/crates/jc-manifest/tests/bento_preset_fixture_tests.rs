//! T-7067: each `spec.http.preset` renders the query parameters, `paginate` option and records
//! split of its API. The fixtures live beside the runner's walk of the same recorded pages
//! (ekiben-io-http tests/fixtures/presets), so the render here and the walk there read one file.

use jc_core::envelope::ResourceEnvelope;
use jc_core::kinds::DataSourceSpec;
use jc_manifest::bento::{input_of, prepended_processors, InputContext};
use std::path::PathBuf;

const PRESETS: [&str; 6] = ["arcgis", "wfs", "ckan", "odata", "sensorthings", "ogc"];

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../crates/ekiben/crates/ekiben-io-http/tests/fixtures/presets")
}

fn context(name: &str) -> InputContext<'_> {
    InputContext {
        source: name,
        project: "bb-ovzdusie",
        pipeline: "presets",
        pipeline_spec: None,
    }
}

fn parse(yaml: &str) -> Result<DataSourceSpec, String> {
    let parsed = ResourceEnvelope::<DataSourceSpec>::from_yaml(yaml).map_err(|e| e.to_string())?;
    parsed.validate().map_err(|e| e.to_string())?;
    Ok(parsed.spec)
}

#[test]
fn each_preset_renders_the_url_and_paginate_option_its_fixture_records() {
    for name in PRESETS {
        let dir = fixtures().join(name);
        let yaml = std::fs::read_to_string(dir.join("source.yaml")).expect("source.yaml");
        let spec = parse(&yaml).unwrap_or_else(|e| panic!("{name}: {e}"));
        let input = serde_json::to_value(input_of(&spec, &context(name))).expect("json");
        let got = serde_json::json!({
            "url": input["http_client"]["url"],
            "paginate": input["http_client"]["paginate"],
        });
        let want: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("render.json")).expect("render.json"),
        )
        .expect("render.json is JSON");
        assert_eq!(
            got, want,
            "{name}: the render no longer matches render.json"
        );
        // The records the walk counts are the records the steps get, one message each.
        let records = want["paginate"]["records"].as_str().expect("records");
        let split =
            serde_json::to_string(&prepended_processors(&spec, &context(name))).expect("json");
        let quoted = records
            .split('.')
            .map(|n| format!(".\\\"{n}\\\""))
            .collect::<String>();
        assert!(
            split.contains(&quoted),
            "{name}: no split of {records}: {split}"
        );
    }
}

fn http(url: &str, preset: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: DataSource\n\
         metadata: {{ name: p, namespace: bb-ovzdusie }}\n\
         spec:\n  type: http\n  http:\n    url: {url}\n    preset: {preset}\n"
    )
}

#[test]
fn a_preset_field_outside_its_schema_is_refused_naming_it() {
    let refused = |url: &str, preset: &str, field: &str| {
        let err = parse(&http(url, preset)).expect_err(preset);
        assert!(err.contains(field), "{preset}: {err} names no {field}");
    };
    let wfs = "https://kartta.hel.fi/ws/geoserver/avoindata/wfs";
    let arcgis = "https://x.example/FeatureServer/0/query";
    let ckan = "https://x.example/api/3/action/package_search";
    // A field no preset has.
    refused(wfs, "{ kind: wfs, typeNames: a:b, sortBy: id }", "sortBy");
    // A field of another kind.
    refused(
        wfs,
        "{ kind: wfs, typeNames: a:b, outFields: [a] }",
        "spec.http.preset.outFields",
    );
    refused(
        ckan,
        "{ kind: ckan, expand: Datastreams }",
        "spec.http.preset.expand",
    );
    // A required field left out, a value outside its grammar.
    refused(wfs, "{ kind: wfs }", "spec.http.preset.typeNames");
    refused(
        arcgis,
        "{ kind: arcgisFeature }",
        "spec.http.preset.outFields",
    );
    refused(
        arcgis,
        "{ kind: arcgisFeature, outFields: ['*'] }",
        "spec.http.preset.outFields",
    );
    refused(
        wfs,
        "{ kind: wfs, typeNames: 'a&b=c' }",
        "spec.http.preset.typeNames",
    );
    refused(
        wfs,
        "{ kind: wfs, typeNames: a:b, pageSize: 0 }",
        "spec.http.preset.pageSize",
    );
    refused(
        wfs,
        "{ kind: wfs, typeNames: a:b, pageSize: 10001 }",
        "spec.http.preset.pageSize",
    );
    refused(wfs, "{ kind: gtfs }", "kind");
    // A URL that is not the API's resource.
    refused(
        "https://x.example/FeatureServer/0",
        "{ kind: arcgisFeature, outFields: [a] }",
        "spec.http.url",
    );
    refused(
        "https://x.example/api/3/action/package_search",
        "{ kind: ckan, resourceId: r1 }",
        "spec.http.url",
    );
    refused(
        "https://x.example/collections/lakes",
        "{ kind: ogcFeatures }",
        "spec.http.url",
    );
    // A preset sets paging and records itself.
    let both = format!(
        "{}    records: features\n",
        http(wfs, "{ kind: wfs, typeNames: a:b }")
    );
    let err = parse(&both).expect_err("records beside a preset");
    assert!(err.contains("spec.http.records"), "{err}");
}

#[test]
fn a_preset_replaces_the_parameters_it_owns_and_keeps_the_rest() {
    let spec = parse(&http(
        "https://x.example/FeatureServer/0/query?f=pjson&where=STATUS%3D1&token=",
        "{ kind: arcgisFeature, outFields: [a, b] }",
    ))
    .expect("valid");
    let input = serde_json::to_value(input_of(&spec, &context("p"))).expect("json");
    assert_eq!(
        input["http_client"]["url"],
        "https://x.example/FeatureServer/0/query?where=STATUS%3D1&token=&outFields=a%2Cb&outSR=4326&f=geojson"
    );
    let spec = parse(&http(
        "https://x.example/api/3/action/datastore_search",
        "{ kind: ckan, resourceId: 5e1e-2f }",
    ))
    .expect("valid");
    let input = serde_json::to_value(input_of(&spec, &context("p"))).expect("json");
    assert_eq!(
        input["http_client"]["url"],
        "https://x.example/api/3/action/datastore_search?resource_id=5e1e-2f"
    );
    assert_eq!(
        input["http_client"]["paginate"]["records"],
        "result.records"
    );
    assert_eq!(input["http_client"]["paginate"]["total"], "result.total");
    let spec = parse(&http(
        "https://x.example/odata/Stops",
        "{ kind: odata, pageSize: 500 }",
    ))
    .expect("valid");
    let input = serde_json::to_value(input_of(&spec, &context("p"))).expect("json");
    assert_eq!(input["http_client"]["paginate"]["offset_param"], "$skip");
    assert_eq!(input["http_client"]["paginate"]["limit_param"], "$top");
}
