// SPDX-License-Identifier: EUPL-1.2
//! What the view loader refuses and what it reports (surfaces-threat-model
//! V16, V27): a view that names nothing to select does not load, and a view
//! that serves history keyed on a member that changes is loaded with a note
//! an operator reads in `/q/health` (ADR-0023 rule 8, the documented
//! residual).

use antares_ops::views::View;
use serde_json::{json, Value};

fn load(doc: &Value) -> Result<antares_ops::views::Loaded, String> {
    serde_json::from_value::<View>(doc.clone())
        .map_err(|e| e.to_string())?
        .load()
}

fn view() -> Value {
    json!({
        "id": "cars", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["query_entities", "temporal_query"]
    })
}

#[test]
fn a_view_without_a_selector_is_refused() {
    let mut empty = view();
    empty["selector"] = json!([]);
    let why = load(&empty).expect_err("an empty selector loads");
    assert!(why.contains("selector"), "{why}");
    let mut absent = view();
    absent.as_object_mut().map(|m| m.remove("selector"));
    assert!(load(&absent).is_err(), "a view without a selector loads");
    // a selector whose types no rule shows selects nothing either
    let mut unshown = view();
    unshown["selector"] = json!(["Truck"]);
    assert!(load(&unshown).is_err(), "a selector no rule shows loads");
    assert!(load(&view()).is_ok());
}

#[test]
fn a_temporal_view_keyed_on_a_mutable_property_is_warned() {
    let rule8 = |l: &antares_ops::views::Loaded| l.notes.iter().any(|n| n.contains("rule 8"));
    let mut keyed = view();
    keyed["q"] = json!("speed<80");
    assert!(
        rule8(&load(&keyed).expect("loads")),
        "a temporal view keyed on speed carries no note"
    );
    // no history served, or no q to key on: nothing to warn about
    let mut current_only = keyed.clone();
    current_only["operations"] = json!(["query_entities"]);
    assert!(!rule8(&load(&current_only).expect("loads")));
    assert!(!rule8(&load(&view()).expect("loads")));
}

/// ADR-0023 rule 12: `selector ["*"]` with the one rule `{types ["*"],
/// attributes ["*"]}` loads; `*` anywhere else is refused, whatever the
/// view's `@context` makes of the term.
#[test]
fn star_loads_alone_and_nowhere_else() {
    let every = json!({
        "id": "all", "tenant": "city", "selector": ["*"],
        "rules": [{"types": ["*"], "attributes": ["*"]}],
        "operations": ["query_entities"]
    });
    assert!(load(&every).is_ok());
    let mut conditioned = every.clone();
    conditioned["q"] = json!("speed<80");
    assert!(load(&conditioned).is_ok(), "the view's own q still narrows");
    let refused = |patch: &dyn Fn(&mut Value)| {
        let mut doc = every.clone();
        patch(&mut doc);
        let why = load(&doc).expect_err("loads");
        assert!(why.contains("rule 12"), "{why}");
    };
    refused(&|d| d["selector"] = json!(["*", "Car"]));
    refused(&|d| d["selector"] = json!(["Car"]));
    refused(&|d| d["rules"][0]["types"] = json!(["Car"]));
    refused(&|d| d["rules"][0]["attributes"] = json!(["speed"]));
    refused(&|d| d["rules"][0]["attributes"] = json!(["*", "speed"]));
    refused(&|d| d["rules"][0]["q"] = json!("speed<80"));
    refused(&|d| d["rules"][0]["scopeQ"] = json!("/a"));
    refused(&|d| {
        d["rules"][0]["geoQ"] =
            json!({"georel": "near;maxDistance==1", "geometry": "Point", "coordinates": [0, 0]});
    });
    refused(&|d| {
        d["rules"] = json!([
            {"types": ["*"], "attributes": ["*"]},
            {"types": ["Car"], "attributes": ["speed"]}
        ]);
    });
    // a named view naming `*` beside its names, and a context defining it
    let mut named = view();
    named["@context"] = json!({"*": "https://uri.etsi.org/ngsi-ld/default-context/Car"});
    named["rules"][0]["attributes"] = json!(["speed", "*"]);
    let why = load(&named).expect_err("a named rule with * loads");
    assert!(why.contains("rule 12"), "{why}");
    named["rules"][0]["attributes"] = json!(["speed"]);
    named["selector"] = json!(["*"]);
    assert!(load(&named).is_err(), "selector * with named rules loads");
}

/// The view T-3501 runs the ETSI suite and surfaces-conformance its MCP suites through loads as
/// committed, in the Tenant `dev/views-rig.sh` seeds.
#[test]
fn the_committed_all_view_loads() {
    let doc: Value = serde_json::from_str(include_str!("../../../ci/views/all.json"))
        .expect("ci/views/all.json is JSON");
    let loaded = load(&doc).expect("ci/views/all.json loads");
    assert!(loaded.view.shows_every_name());
    assert_eq!(loaded.tenant, "conformance");
}

/// The view E4 runs the ETSI suite through (T-3787) loads as committed: Building and City, every
/// Attribute the suite writes on them but `airQualityLevel`, the read operations only.
#[test]
fn the_committed_etsi_restricted_view_loads() {
    let doc: Value =
        serde_json::from_str(include_str!("../../../ci/views/etsi/etsi-restricted.json"))
            .expect("ci/views/etsi/etsi-restricted.json is JSON");
    let loaded = load(&doc).expect("ci/views/etsi/etsi-restricted.json loads");
    assert!(!loaded.view.shows_every_name());
    let (types, attributes) = loaded.view.shown_names();
    assert_eq!(
        types,
        ["City", "https://ngsi-ld-test-suite/context#Building"],
        "compacted under the core @context"
    );
    assert!(attributes.contains(&"https://ngsi-ld-test-suite/context#name".to_owned()));
    assert!(!attributes.iter().any(|a| a.ends_with("airQualityLevel")));
    assert_eq!(
        loaded.operations.len(),
        15,
        "the read operations of operation-matrix.yaml"
    );
}
