// SPDX-License-Identifier: EUPL-1.2
//! `expand_entity` over the ETSI NGSI-LD test suite's own Entity fixtures
//! (`ngsi-ld-test-suite/data/entities`, vendored in the workspace; not in the published crate).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use ngsild_jsonld::{core_context, expand_entity, Context, ExpandOpts};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

fn suite() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ngsi-ld-test-suite")
}

/// The core `@context` plus the suite's own terms, as a broker resolves the fixtures' link.
fn suite_context() -> Context {
    let raw = std::fs::read_to_string(
        suite().join("resources/jsonld-contexts/ngsi-ld-test-suite.jsonld"),
    )
    .unwrap();
    let doc: Value = serde_json::from_str(&raw).unwrap();
    let mut ctx = core_context();
    ctx.merge_object(doc["@context"].as_object().unwrap())
        .unwrap();
    ctx.freeze();
    ctx
}

/// Every fixture that is a JSON object, `@context` removed; Robot-templated or
/// deliberately broken JSON (`invalid-json`, `empty`) is not an Entity and is skipped.
fn fixtures() -> Vec<(String, Map<String, Value>)> {
    let mut out: Vec<_> = std::fs::read_dir(suite().join("data/entities"))
        .unwrap()
        .filter_map(|e| {
            let path = e.unwrap().path();
            let Ok(Value::Object(mut doc)) =
                serde_json::from_str(&std::fs::read_to_string(&path).ok()?)
            else {
                return None;
            };
            doc.remove("@context");
            Some((path.file_name()?.to_string_lossy().into_owned(), doc))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The suite names its negative fixtures; each must be refused with a 400,
/// every other Entity, fragment or attribute body expands.
#[test]
fn the_suite_fixtures_expand_and_its_negative_ones_are_refused() {
    let ctx = suite_context();
    let all = fixtures();
    assert!(
        all.len() > 70,
        "only {} fixtures read from {}",
        all.len(),
        suite().display()
    );
    let (mut ok, mut refused) = (0, 0);
    for (name, mut doc) in all {
        // The suite's Robot keywords replace this placeholder with a fresh URN.
        if doc.get("id").and_then(Value::as_str) == Some("RandomUuidToBeReplaced") {
            doc.insert("id".into(), "urn:ngsi-ld:Building:fixture".into());
        }
        // A body of `PUT /entities/{id}/attrs/{attr}` is one attribute: expand it as a fragment holding it.
        if !doc.contains_key("id") && doc.contains_key("type") {
            doc = Map::from_iter([("speed".to_owned(), Value::Object(doc))]);
        }
        let negative = ["invalid-", "meta-model-", "temporal-observedat-"]
            .iter()
            .any(|p| name.starts_with(p));
        let opts = ExpandOpts {
            fragment: !doc.contains_key("id"),
            ..ExpandOpts::default()
        };
        let out = expand_entity(&doc, &ctx, opts);
        assert_eq!(out.is_err(), negative, "{name}: {out:?}");
        if let Err(e) = out {
            assert_eq!(e.status(), 400, "{name}: {e}");
            assert_eq!(e.to_problem_details().title, "BadRequestData", "{name}");
            refused += 1;
        } else {
            ok += 1;
        }
    }
    assert!(ok > 60, "{ok} expanded");
    assert!(refused >= 8, "{refused} refused");
}
