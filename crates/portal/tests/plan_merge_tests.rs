//! The three-way merge a workspace is updated from main with (CC-80, T-1237).

use joinedcontext_portal::plan::{merge3, Side};
use serde_json::json;

#[test]
fn one_side_changing_a_field_takes_that_side() {
    let base = json!({ "spec": { "a": 1, "b": 1 } });
    let ours = json!({ "spec": { "a": 2, "b": 1 } });
    let theirs = json!({ "spec": { "a": 1, "b": 3, "c": 4 } });
    let merged = merge3(Some(&base), &ours, &theirs, &|_| None).unwrap();
    assert_eq!(merged, json!({ "spec": { "a": 2, "b": 3, "c": 4 } }));
}

#[test]
fn both_sides_changing_a_field_alike_is_no_conflict() {
    let base = json!({ "a": 1 });
    let same = json!({ "a": 2 });
    assert_eq!(merge3(Some(&base), &same, &same, &|_| None).unwrap(), same);
}

#[test]
fn a_field_both_changed_differently_is_listed_and_no_side_wins() {
    let base = json!({ "spec": { "period": "10s", "list": [1] } });
    let ours = json!({ "spec": { "period": "30s", "list": [1, 2] } });
    let theirs = json!({ "spec": { "period": "60s", "list": [3] } });
    let conflicts = merge3(Some(&base), &ours, &theirs, &|_| None).unwrap_err();
    let paths: Vec<_> = conflicts.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["spec.list", "spec.period"], "an array is one value");
    assert_eq!(conflicts[1].ours, json!("30s"));
    assert_eq!(conflicts[1].theirs, json!("60s"));
    assert_eq!(conflicts[1].base, json!("10s"));
}

#[test]
fn a_persons_answer_decides_each_conflict() {
    let base = json!({ "a": 1, "b": 1 });
    let ours = json!({ "a": 2, "b": 2 });
    let theirs = json!({ "a": 3, "b": 3 });
    let pick = |path: &str| match path {
        "a" => Some(Side::Ours),
        "b" => Some(Side::Theirs),
        _ => None,
    };
    assert_eq!(
        merge3(Some(&base), &ours, &theirs, &pick).unwrap(),
        json!({ "a": 2, "b": 3 })
    );
}

#[test]
fn a_field_removed_on_one_side_and_changed_on_the_other_is_a_conflict() {
    let base = json!({ "a": 1, "keep": true });
    let ours = json!({ "keep": true });
    let theirs = json!({ "a": 5, "keep": true });
    let conflicts = merge3(Some(&base), &ours, &theirs, &|_| None).unwrap_err();
    assert_eq!(conflicts[0].path, "a");
    assert_eq!(conflicts[0].ours, serde_json::Value::Null);
    let taken = merge3(Some(&base), &ours, &theirs, &|_| Some(Side::Ours)).unwrap();
    assert_eq!(taken, json!({ "keep": true }), "removal kept");
}

#[test]
fn with_no_base_both_additions_of_the_same_key_differ() {
    let conflicts = merge3(None, &json!({ "a": 1 }), &json!({ "a": 2 }), &|_| None).unwrap_err();
    assert_eq!(conflicts.len(), 1);
    assert!(
        merge3(None, &json!({}), &json!({}), &|_| None).is_ok(),
        "empty documents"
    );
}
