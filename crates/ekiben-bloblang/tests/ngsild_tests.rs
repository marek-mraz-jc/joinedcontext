//! E7: the NGSI-LD helpers, run through the interpreter as a mapping author writes them.

use ekiben_bloblang::{Environment, Value};
use serde_json::{json, Value as JsonValue};

fn run(mapping: &str, input: JsonValue) -> Result<JsonValue, String> {
    let exec = Environment::new()
        .compile_mapping(mapping)
        .map_err(|e| format!("parse: {e}"))?;
    exec.exec_on_value(Value::from_json_value(input))
        .map(|v| v.to_json_value())
        .map_err(|e| e.to_string())
}

fn ok(mapping: &str, input: JsonValue) -> JsonValue {
    match run(mapping, input) {
        Ok(v) => v,
        Err(e) => panic!("{mapping}: {e}"),
    }
}

fn err(mapping: &str, input: JsonValue) -> String {
    match run(mapping, input) {
        Ok(v) => panic!("{mapping} must fail, got {v}"),
        Err(e) => e,
    }
}

/// The Property, Relationship, GeoProperty and LanguageProperty of ETSI GS CIM 009's examples.
fn normalized_example() -> JsonValue {
    json!({
        "id": "urn:ngsi-ld:OffStreetParking:Downtown1",
        "type": "OffStreetParking",
        "name": {"type": "Property", "value": "Downtown One"},
        "availableSpotNumber": {"type": "Property", "value": 121},
        "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [-8.5, 41.2]}},
        "refProvider": {"type": "Relationship", "object": "urn:ngsi-ld:Company:A"},
        "street": {"type": "LanguageProperty", "languageMap": {"fr": "Grand Place", "nl": "Grote Markt"}}
    })
}

fn simplified_example() -> JsonValue {
    json!({
        "id": "urn:ngsi-ld:OffStreetParking:Downtown1",
        "type": "OffStreetParking",
        "name": "Downtown One",
        "availableSpotNumber": 121,
        "location": {"type": "Point", "coordinates": [-8.5, 41.2]},
        "refProvider": "urn:ngsi-ld:Company:A",
        "street": {"languageMap": {"fr": "Grand Place", "nl": "Grote Markt"}}
    })
}

// --- ngsild_property ---

#[test]
fn property_examples() {
    assert_eq!(
        ok(
            r#"root = this.t.ngsild_property(unit_code: "CEL", observed_at: "2026-01-01T00:00:00Z")"#,
            json!({"t": 21.5})
        ),
        json!({"type": "Property", "value": 21.5, "unitCode": "CEL", "observedAt": "2026-01-01T00:00:00Z"})
    );
    assert_eq!(
        ok("root = this.t.ngsild_property()", json!({"t": "on"})),
        json!({"type": "Property", "value": "on"})
    );
    assert_eq!(
        ok(
            r#"root = this.t.ngsild_property("", null, "urn:ngsi-ld:Dataset:a")"#,
            json!({"t": [1, 2]})
        ),
        json!({"type": "Property", "value": [1, 2], "datasetId": "urn:ngsi-ld:Dataset:a"})
    );
    assert_eq!(
        ok(
            r#"root = this.t.ngsild_property(observed_at: "2026-01-01T01:00:00+01:00".ts_parse("2006-01-02T15:04:05Z07:00"))"#,
            json!({"t": 1})
        ),
        json!({"type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00Z"})
    );
}

#[test]
fn property_null_empty_and_types() {
    assert!(err("root = this.missing.ngsild_property()", json!({})).contains("needs a value"));
    assert!(err(
        r#"root = this.t.ngsild_property(observed_at: "yesterday")"#,
        json!({"t": 1})
    )
    .contains("RFC 3339"));
    assert!(err(
        "root = this.t.ngsild_property(unit_code: 5)",
        json!({"t": 1})
    )
    .contains("expected string value"));
    assert!(err(
        r#"root = this.t.ngsild_property(dataset_id: "plain")"#,
        json!({"t": 1})
    )
    .contains("not a URI"));
}

// --- ngsild_relationship ---

#[test]
fn relationship_examples() {
    assert_eq!(
        ok(
            "root = this.owner.ngsild_relationship()",
            json!({"owner": "urn:ngsi-ld:Person:1"})
        ),
        json!({"type": "Relationship", "object": "urn:ngsi-ld:Person:1"})
    );
    assert_eq!(
        ok(
            r#"root = this.o.ngsild_relationship(observed_at: "2026-01-01T00:00:00Z", dataset_id: "urn:ds:1")"#,
            json!({"o": ["urn:ngsi-ld:A:1", "urn:ngsi-ld:A:2"]})
        ),
        json!({"type": "Relationship", "object": ["urn:ngsi-ld:A:1", "urn:ngsi-ld:A:2"], "observedAt": "2026-01-01T00:00:00Z", "datasetId": "urn:ds:1"})
    );
}

#[test]
fn relationship_null_empty_and_types() {
    assert!(err("root = this.o.ngsild_relationship()", json!({"o": ""})).contains("empty"));
    assert!(err(
        "root = this.o.ngsild_relationship()",
        json!({"o": "not a uri"})
    )
    .contains("URI"));
    assert!(err("root = this.o.ngsild_relationship()", json!({"o": []})).contains("URI"));
    let e = err("root = this.o.ngsild_relationship()", json!({"o": 5}));
    assert!(e.contains("expected string value, got number"), "{e}");
    assert!(err("root = this.o.ngsild_relationship()", json!({}))
        .contains("expected string value, got null"));
}

// --- ngsild_geoproperty ---

#[test]
fn geoproperty_examples() {
    assert_eq!(
        ok(
            r#"root = this.g.ngsild_geoproperty(observed_at: "2026-01-01T00:00:00Z")"#,
            json!({"g": {"type": "Point", "coordinates": [14.42, 50.08]}})
        ),
        json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [14.42, 50.08]}, "observedAt": "2026-01-01T00:00:00Z"})
    );
    let polygon = json!({"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 0]]]});
    assert_eq!(
        ok(
            "root = this.g.ngsild_geoproperty()",
            json!({"g": polygon.clone()})
        ),
        json!({"type": "GeoProperty", "value": polygon})
    );
    let collection = json!({"type": "GeometryCollection", "geometries": [
        {"type": "LineString", "coordinates": [[0, 0], [1, 1]]},
        {"type": "MultiPoint", "coordinates": [[0, 0]]}
    ]});
    assert!(
        ok(
            "root = this.g.ngsild_geoproperty()",
            json!({"g": collection})
        )["type"]
            == "GeoProperty"
    );
}

#[test]
fn geoproperty_validates_type_and_nesting() {
    for (g, needle) in [
        (
            json!({"type": "Point", "coordinates": [1]}),
            "2 or 3 numbers",
        ),
        (
            json!({"type": "Point", "coordinates": [[1, 2]]}),
            "numbers only",
        ),
        (
            json!({"type": "Point", "coordinates": [200, 0]}),
            "outside longitude",
        ),
        (
            json!({"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 1]]]}),
            "end where it starts",
        ),
        (
            json!({"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [0, 0]]]}),
            "at least 4",
        ),
        (
            json!({"type": "LineString", "coordinates": [[0, 0]]}),
            "at least 2",
        ),
        (
            json!({"type": "Circle", "coordinates": [0, 0]}),
            "not a GeoJSON geometry type",
        ),
        (json!({"coordinates": [0, 0]}), "needs a type"),
        (json!({"type": "Point"}), "needs coordinates"),
    ] {
        let e = err("root = this.g.ngsild_geoproperty()", json!({"g": g}));
        assert!(e.contains(needle), "{e} lacks {needle}");
    }
    let e = err(
        "root = this.g.ngsild_geoproperty()",
        json!({"g": "POINT(1 2)"}),
    );
    assert!(e.contains("expected object value, got string"), "{e}");
}

// --- ngsild_language_property ---

#[test]
fn language_property_examples() {
    assert_eq!(
        ok(
            r#"root = this.s.ngsild_language_property(lang: "en")"#,
            json!({"s": "Main Street"})
        ),
        json!({"type": "LanguageProperty", "languageMap": {"en": "Main Street"}})
    );
    assert_eq!(
        ok(
            "root = this.s.ngsild_language_property()",
            json!({"s": {"cs": "Hlavní", "en": "Main"}})
        ),
        json!({"type": "LanguageProperty", "languageMap": {"cs": "Hlavní", "en": "Main"}})
    );
}

#[test]
fn language_property_null_empty_and_types() {
    assert!(err(
        "root = this.s.ngsild_language_property()",
        json!({"s": "x"})
    )
    .contains("only with lang"));
    assert!(err("root = this.s.ngsild_language_property()", json!({"s": {}})).contains("empty"));
    assert!(err(
        r#"root = this.s.ngsild_language_property(lang: "e n")"#,
        json!({"s": "x"})
    )
    .contains("not a language tag"));
    assert!(err(
        "root = this.s.ngsild_language_property()",
        json!({"s": {"en": 1}})
    )
    .contains("holding a string"));
    let e = err(
        r#"root = this.s.ngsild_language_property(lang: "en")"#,
        json!({"s": 3}),
    );
    assert!(e.contains("expected string value, got number"), "{e}");
}

// --- ngsild_key_values / ngsild_normalized ---

#[test]
fn key_values_example() {
    assert_eq!(
        ok("root = this.ngsild_key_values()", normalized_example()),
        simplified_example()
    );
    assert_eq!(
        ok(
            "root = this.ngsild_key_values()",
            json!({"id": "urn:a", "type": "T",
                   "j": {"type": "JsonProperty", "json": {"k": 1}},
                   "v": {"type": "VocabProperty", "vocab": "x"},
                   "l": {"type": "ListProperty", "valueList": [1, 2]},
                   "m": [{"type": "Property", "value": 1, "datasetId": "urn:ds:1"}, {"type": "Property", "value": 2}]})
        ),
        json!({"id": "urn:a", "type": "T", "j": {"json": {"k": 1}}, "v": {"vocab": "x"}, "l": [1, 2],
               "m": {"dataset": {"urn:ds:1": 1, "@none": 2}}})
    );
}

#[test]
fn normalized_from_the_simplified_example() {
    let got = ok("root = this.ngsild_normalized()", simplified_example());
    assert_eq!(got, normalized_example());
    // A ref key that holds no URN stays a Property; an id is kept as it is.
    assert_eq!(
        ok(
            "root = this.ngsild_normalized()",
            json!({"id": "urn:a", "type": "T", "refCode": "ABC", "owner": "urn:ngsi-ld:P:1"})
        ),
        json!({"id": "urn:a", "type": "T",
               "refCode": {"type": "Property", "value": "ABC"},
               "owner": {"type": "Property", "value": "urn:ngsi-ld:P:1"}})
    );
}

#[test]
fn conversions_null_and_types() {
    assert!(err(
        "root = this.ngsild_normalized()",
        json!({"id": "urn:a", "type": "T", "x": null})
    )
    .contains("null"));
    let e = err("root = this.ngsild_normalized()", json!([1]));
    assert!(e.contains("expected object value, got array"), "{e}");
    let e = err("root = this.ngsild_key_values()", json!("text"));
    assert!(e.contains("expected object value, got string"), "{e}");
    assert_eq!(ok("root = this.ngsild_key_values()", json!({})), json!({}));
}

#[test]
fn round_trip_normalized_key_values_normalized() {
    let back = ok(
        "root = this.ngsild_key_values().ngsild_normalized()",
        normalized_example(),
    );
    assert_eq!(back, normalized_example());
    let multi = json!({"id": "urn:a", "type": "T",
        "m": [{"type": "Property", "value": 1, "datasetId": "urn:ds:1"}, {"type": "Property", "value": 2}]});
    let back = ok("root = this.ngsild_key_values().ngsild_normalized()", multi);
    let items = back["m"].as_array().cloned().unwrap_or_default();
    assert_eq!(items.len(), 2);
    assert!(items.contains(&json!({"type": "Property", "value": 1, "datasetId": "urn:ds:1"})));
    assert!(items.contains(&json!({"type": "Property", "value": 2})));
}

// --- ngsild_urn ---

#[test]
fn urn_examples() {
    assert_eq!(
        ok(r#"root = ngsild_urn("Vehicle", "B-123")"#, json!({})),
        json!("urn:ngsi-ld:Vehicle:B-123")
    );
    assert_eq!(
        ok(
            r#"root = ngsild_urn(type: "Vehicle", id: this.n, prefix: "praha:")"#,
            json!({"n": 7})
        ),
        json!("urn:ngsi-ld:Vehicle:praha:7")
    );
    assert_eq!(
        ok(r#"root = ngsild_urn("Stop", "A 1/b%?#é")"#, json!({})),
        json!("urn:ngsi-ld:Stop:A%201/b%25%3F%23%C3%A9")
    );
}

#[test]
fn urn_null_empty_and_types() {
    assert!(err(r#"root = ngsild_urn("Vehicle", "")"#, json!({})).contains("must not be empty"));
    assert!(err(r#"root = ngsild_urn("Vehicle", this.x)"#, json!({}))
        .contains("expected string value, got null"));
    assert!(err(r#"root = ngsild_urn("Veh icle", "1")"#, json!({})).contains("parameter type"));
    assert!(err(r#"root = ngsild_urn(5, "1")"#, json!({})).contains("expected string value"));
    assert!(err(r#"root = ngsild_urn("Vehicle")"#, json!({})).contains("missing parameter: id"));
}

// --- the extension names ---

#[test]
fn the_extension_names_are_known_to_the_parser() {
    let env = Environment::new();
    let functions = env.function_names();
    let methods = env.method_names();
    for f in ekiben_bloblang::ngsild::EXTENSION_FUNCTIONS {
        assert!(functions.iter().any(|n| n == f), "{f}");
    }
    for m in ekiben_bloblang::ngsild::EXTENSION_METHODS {
        assert!(methods.iter().any(|n| n == m), "{m}");
    }
}

#[test]
fn no_value_makes_a_helper_panic() {
    let mut deep = json!({"type": "Point", "coordinates": [0, 0]});
    for _ in 0..40 {
        deep = json!({"type": "GeometryCollection", "geometries": [deep]});
    }
    let values = [
        json!(null),
        json!(true),
        json!(0),
        json!(-1.5e308),
        json!(18446744073709551615u64),
        json!(""),
        json!("é\u{0}"),
        json!([]),
        json!([[[[[]]]]]),
        json!({}),
        json!({"type": 5}),
        json!({"type": "Polygon", "coordinates": [[]]}),
        json!({"type": "MultiPolygon", "coordinates": [[[["x"]]]]}),
        json!({"id": [], "type": {}, "a": {"dataset": {"@none": {"dataset": {}}}}}),
        deep,
    ];
    let calls = [
        "root = this.v.ngsild_property()",
        "root = this.v.ngsild_relationship()",
        "root = this.v.ngsild_geoproperty()",
        "root = this.v.ngsild_language_property()",
        r#"root = this.v.ngsild_language_property(lang: "en")"#,
        "root = this.v.ngsild_key_values()",
        "root = this.v.ngsild_normalized()",
        r#"root = ngsild_urn("T", this.v)"#,
    ];
    for v in &values {
        for c in calls {
            let _ = run(c, json!({"v": v}));
        }
    }
}
