//! T-7793: what `apply` does to one declared seed entity given what the broker answered (CC-07):
//! only declared attributes count, the broker's own timestamps never do.

use jcctl::entities::{action, seed_entities, Action, SeedError};
use serde_json::json;

#[test]
fn an_entity_the_broker_does_not_hold_is_created() {
    assert_eq!(
        action(&json!({ "id": "urn:ngsi-ld:A:1" }), None),
        Action::Create
    );
}

#[test]
fn a_declared_or_live_value_that_is_not_an_object_is_an_update() {
    assert_eq!(action(&json!([1]), Some(&json!({}))), Action::Update);
    assert_eq!(
        action(&json!({ "a": 1 }), Some(&json!("text"))),
        Action::Update
    );
}

#[test]
fn broker_timestamps_at_any_depth_are_not_drift_but_a_missing_or_changed_attribute_is() {
    let declared = json!({
        "id": "urn:ngsi-ld:A:1",
        "type": "A",
        "createdAt": "2020-01-01T00:00:00Z",
        "temp": { "type": "Property", "value": 20, "modifiedAt": "2020-01-01T00:00:00Z" },
        "tags": { "type": "Property", "value": ["a", "b"] }
    });
    let live = json!({
        "id": "urn:ngsi-ld:A:1",
        "type": "A",
        "createdAt": "2026-11-24T00:00:00Z",
        "temp": { "type": "Property", "value": 20, "instanceId": "urn:x", "observedAt": "now" },
        "tags": { "type": "Property", "value": ["a", "b"] },
        "measured": { "type": "Property", "value": 3 }
    });
    assert_eq!(action(&declared, Some(&live)), Action::Unchanged);

    let mut shorter = live.clone();
    shorter["tags"]["value"] = json!(["a"]);
    assert_eq!(
        action(&declared, Some(&shorter)),
        Action::Update,
        "a list of another length"
    );

    let mut reordered = live.clone();
    reordered["tags"]["value"] = json!(["b", "a"]);
    assert_eq!(
        action(&declared, Some(&reordered)),
        Action::Update,
        "order is part of a list"
    );

    let mut gone = live.clone();
    gone.as_object_mut().expect("object").remove("temp");
    assert_eq!(
        action(&declared, Some(&gone)),
        Action::Update,
        "a declared attribute is missing"
    );

    let mut changed = live;
    changed["temp"]["value"] = json!(21);
    assert_eq!(action(&declared, Some(&changed)), Action::Update);
}

#[test]
fn a_checkout_that_is_not_there_is_unreadable_and_names_its_path() {
    let missing = std::env::temp_dir().join(format!("jcctl-no-checkout-{}", std::process::id()));
    match seed_entities(&missing) {
        Err(SeedError::Unreadable { path, message }) => {
            assert!(
                path.ends_with(&format!("jcctl-no-checkout-{}", std::process::id())),
                "{path}"
            );
            assert!(!message.is_empty());
        }
        other => panic!("expected Unreadable, got {other:?}"),
    }
}
