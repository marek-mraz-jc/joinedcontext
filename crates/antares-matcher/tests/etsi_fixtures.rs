// SPDX-License-Identifier: EUPL-1.2
//! `would_notify` over the ETSI NGSI-LD test suite's own Subscription and Entity fixtures
//! (`ngsi-ld-test-suite`, vendored in the workspace; not in the published crate).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use antares_jsonld::{core_context, expand_entity, Context, ExpandOpts};
use ngsild_matcher::would_notify;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn suite() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ngsi-ld-test-suite")
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn suite_context() -> Context {
    let doc =
        read_json(&suite().join("resources/jsonld-contexts/ngsi-ld-test-suite.jsonld")).unwrap();
    let mut ctx = core_context();
    ctx.merge_object(doc["@context"].as_object().unwrap())
        .unwrap();
    ctx.freeze();
    ctx
}

fn entity(name: &str, ctx: &Context) -> Value {
    let Some(Value::Object(mut doc)) = read_json(&suite().join("data/entities").join(name)) else {
        panic!("{name}")
    };
    doc.remove("@context");
    expand_entity(&doc, ctx, ExpandOpts::default()).unwrap()
}

/// The suite Subscriptions that select by Entity type alone (no q, geoQ, scopeQ or
/// watchedAttributes, active, not expiring) notify for a suite Entity of that type and for no
/// other; their selector type is expanded first, as a broker stores it.
#[test]
fn a_type_only_suite_subscription_notifies_for_its_type_and_no_other() {
    let ctx = suite_context();
    let building = entity("building-minimal.jsonld", &ctx);
    let vehicle = entity("vehicle-speed-attribute.jsonld", &ctx);
    let mut files: Vec<_> = std::fs::read_dir(suite().join("data/subscriptions"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let mut checked = 0;
    for path in files {
        let Some(Value::Object(mut sub)) = read_json(&path) else {
            continue;
        };
        let filtered = [
            "q",
            "geoQ",
            "watchedAttributes",
            "scopeQ",
            "expiresAt",
            "temporalQ",
        ]
        .iter()
        .any(|k| sub.contains_key(*k))
            || sub.get("isActive") == Some(&Value::Bool(false));
        let Some(Value::Array(selectors)) = sub.get_mut("entities") else {
            continue;
        };
        let [Value::Object(sel)] = selectors.as_mut_slice() else {
            continue;
        };
        let Some(Value::String(t)) = sel.get("type").cloned() else {
            continue;
        };
        if filtered || sel.len() != 1 || !["Building", "Vehicle"].contains(&t.as_str()) {
            continue;
        }
        sel.insert("type".into(), ctx.expand_key(&t).into());
        let sub = Value::Object(sub);
        let (hit, miss) = if t == "Building" {
            (&building, &vehicle)
        } else {
            (&vehicle, &building)
        };
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            would_notify(&sub, hit, &ctx, &|_| None),
            "{name}: no notification for a {t}"
        );
        assert!(
            !would_notify(&sub, miss, &ctx, &|_| None),
            "{name}: notified for another type"
        );
        checked += 1;
    }
    assert!(checked >= 20, "only {checked} suite subscriptions checked");
}
