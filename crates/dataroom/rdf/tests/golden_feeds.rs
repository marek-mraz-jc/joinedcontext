//! The feeds of a record that declares everything, and of the national specifications' own
//! examples, byte for byte (`tests/golden/`).

use std::{collections::BTreeMap, fs, path::Path};

use dataroom_core::{Catalogue, Dataset};
use dataroom_rdf::{
    parse_datasets_from_rdf, serialize_catalogue, serialize_dataset, DcatProfile, RdfFormat,
    RdfLimits,
};

fn records() -> (Catalogue, Vec<(&'static str, Dataset)>) {
    let mut fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/complete.json")).expect("fixture");
    let catalogue: Catalogue =
        serde_json::from_value(fixture["catalogue"].take()).expect("catalogue");
    let complete: Dataset = serde_json::from_value(fixture["dataset"].take()).expect("dataset");
    let mut out = vec![("complete", complete)];
    for (name, ttl, lang) in [
        (
            "cz-example",
            include_str!("../../specs/dcat-ap-cz/priklady-katalog/datova-sada.ttl"),
            "cs",
        ),
        (
            "sk-example",
            include_str!("../../specs/dcat-ap-sk/príklady/dataset.ttl"),
            "sk",
        ),
    ] {
        let mut parsed = parse_datasets_from_rdf(
            ttl,
            RdfFormat::Turtle,
            catalogue.org_id,
            catalogue.id,
            "city",
            lang,
            &RdfLimits::default(),
        )
        .expect("a published example parses")
        .remove(0);
        parsed.issued = catalogue.modified;
        parsed.modified = catalogue.modified;
        out.push((name, parsed));
    }
    (catalogue, out)
}

fn json_of(text: &str) -> Option<serde_json::Value> {
    serde_json::from_str(text).ok()
}

#[test]
fn feeds_match_their_golden_files() {
    let (catalogue, datasets) = records();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let actual_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("golden");
    fs::create_dir_all(&actual_dir).expect("scratch directory");
    let mut differs = Vec::new();
    for (label, profile) in [
        ("eu", DcatProfile::eu()),
        (
            "cz",
            DcatProfile::from_token("dcat-ap-cz").expect("installed"),
        ),
        (
            "sk",
            DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
        ),
    ] {
        for (extension, format) in [("ttl", RdfFormat::Turtle), ("jsonld", RdfFormat::JsonLd)] {
            let mut feeds: Vec<(String, String)> = datasets
                .iter()
                .map(|(name, ds)| {
                    (
                        format!("{label}-{name}.{extension}"),
                        serialize_dataset(ds, &catalogue, None, format, profile),
                    )
                })
                .collect();
            let all: Vec<Dataset> = datasets.iter().map(|(_, d)| d.clone()).collect();
            feeds.push((
                format!("{label}-catalogue.{extension}"),
                serialize_catalogue(&catalogue, &all, &BTreeMap::new(), format, profile, None),
            ));
            for (file, actual) in feeds {
                let golden = fs::read_to_string(dir.join(&file)).ok();
                // A JSON object's key order follows serde_json's unified `preserve_order`.
                let same = if extension == "jsonld" {
                    golden.as_deref().and_then(json_of) == json_of(&actual)
                } else {
                    golden.as_ref() == Some(&actual)
                };
                if !same {
                    fs::write(actual_dir.join(&file), &actual).expect("write the differing feed");
                    differs.push(file);
                }
            }
        }
    }
    assert!(
        differs.is_empty(),
        "{differs:#?} differ from tests/golden; the rendered files are in {}",
        actual_dir.display()
    );
}
