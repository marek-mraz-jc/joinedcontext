//! G-16: the NGSI-LD helper functions `ngsild_ref`, `ngsild_point`, `ngsild_multi` and
//! `ngsild_language_map`.

use std::collections::HashMap;

use ekiben_bloblang::{Environment, Value};
use serde_json::json;

fn run_in(env: Environment, mapping: &str) -> Result<serde_json::Value, String> {
    let exec = env.compile_mapping(mapping).map_err(|e| e.to_string())?;
    let v = exec
        .exec_on_value(Value::from_json_value(json!({})))
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&v.to_go_json()).map_err(|e| e.to_string())
}

fn run(mapping: &str) -> Result<serde_json::Value, String> {
    let env = Environment::new().with_env_vars(HashMap::from([
        ("JC_ORG_ID".to_string(), "sk_bbsk".to_string()),
        ("JC_SPACE".to_string(), "bbsk-air".to_string()),
    ]));
    run_in(env, mapping)
}

#[test]
fn ref_mints_the_pf42_urn_from_the_injected_owner_and_space() {
    assert_eq!(
        run(r#"root = ngsild_ref("Station", "A 1")"#).expect("ref"),
        json!("urn:ngsi-ld:Station:sk_bbsk:bbsk-air:A%201")
    );
    assert_eq!(
        run(r#"root = ngsild_ref("Station", 7, "other")"#).expect("ref"),
        json!("urn:ngsi-ld:Station:sk_bbsk:other:7")
    );
    let missing = run_in(Environment::new(), r#"root = ngsild_ref("Station", "a")"#).unwrap_err();
    assert!(missing.contains("JC_"), "{missing}");
    assert!(run(r#"root = ngsild_ref("Sta tion", "a")"#).is_err());
    assert!(run(r#"root = ngsild_ref("Station", "")"#).is_err());
}

#[test]
fn point_checks_ranges_and_spots_a_swap() {
    assert_eq!(
        run(r#"root = ngsild_point(48.7, "19.1")"#).expect("point"),
        json!({"type": "Point", "coordinates": [19.1, 48.7]})
    );
    let lat = run("root = ngsild_point(91, 10)").unwrap_err();
    assert!(lat.contains("latitude is out of range"), "{lat}");
    let swapped = run("root = ngsild_point(120.5, 48.7)").unwrap_err();
    assert!(swapped.contains("swapped"), "{swapped}");
    let lon = run("root = ngsild_point(48.7, 190.5)").unwrap_err();
    assert!(lon.contains("longitude"), "{lon}");
    assert!(run(r#"root = ngsild_point("NaN", 1.5)"#).is_err());
    assert!(run(r#"root = ngsild_point(48.7, 19.1, "EPSG:3879")"#).is_err());
    assert!(run(r#"root = ngsild_point(48.7, 19.1, "EPSG:4326")"#).is_ok());
}

#[test]
fn multi_wraps_instances_with_their_dataset_ids() {
    assert_eq!(
        run(r#"root = ngsild_multi({"urn:ngsi-ld:Dataset:a": {"type": "Property", "value": 1}, "@none": {"type": "Property", "value": 2, "datasetId": "urn:x:old"}})"#)
            .expect("multi"),
        json!([
            {"type": "Property", "value": 2},
            {"type": "Property", "value": 1, "datasetId": "urn:ngsi-ld:Dataset:a"}
        ])
    );
    assert!(run(r#"root = ngsild_multi({"not a uri": {"type": "Property", "value": 1}})"#).is_err());
    assert!(run(r#"root = ngsild_multi({"urn:x:a": 5})"#).is_err());
    assert!(run(r#"root = ngsild_multi({})"#).is_err());
}

#[test]
fn language_map_drops_empty_texts() {
    assert_eq!(
        run(r#"root.name = ngsild_language_map({"fi": "Kaivopuisto", "sv": "", "en": null, "sk": ["a", ""]})"#)
            .expect("map"),
        json!({"name": {"type": "LanguageProperty", "languageMap": {"fi": "Kaivopuisto", "sk": ["a"]}}})
    );
    assert_eq!(
        run(r#"root.a = 1
root.name = ngsild_language_map({"fi": ""})"#)
            .expect("nothing left"),
        json!({"a": 1})
    );
    assert!(run(r#"root = ngsild_language_map({"en_US": "x"})"#).is_err());
    assert!(run(r#"root = ngsild_language_map({"en": 3})"#).is_err());
}

#[test]
fn no_argument_is_an_error_never_a_panic() {
    for f in ["ngsild_ref", "ngsild_point", "ngsild_multi", "ngsild_language_map"] {
        assert!(run(&format!("root = {f}()")).is_err(), "{f}");
        assert!(run(&format!("root = {f}(null)")).is_err(), "{f}");
    }
}
