//! Edge cases of the write guards through the public API (T-7805): `Link` headers that are no
//! text or carry no parameters, bodies whose top level is no object, contexts hidden in lists,
//! schemas that state no unit, every refusal's words, and each value form a quantity may take.

use http::header::LINK;
use http::{HeaderMap, HeaderValue};
use jc_core::kinds::MissingUnitCode;
use jc_core::ProblemDetails;
use jc_ngsi_input::context_guard::{check_body, check_link, is_core_context, REFUSAL};
use jc_ngsi_input::units::{enforce, Shape, UnitRefusal, UnitRules};
use serde_json::{json, Value};

const CORE: &str = "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context.jsonld";

fn links(values: &[&[u8]]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for v in values {
        headers.append(LINK, HeaderValue::from_bytes(v).expect("a header value"));
    }
    headers
}

#[test]
fn a_link_that_is_not_text_is_refused() {
    // Obsolete-text bytes are a valid header value but no UTF-8 string `to_str` reads.
    assert_eq!(
        check_link(&links(&[b"<https://example.org/\xff>; rel=x"])),
        Err(REFUSAL)
    );
    // Behind an accepted core link the second value is still read.
    let core = format!("<{CORE}>; rel=\"http://www.w3.org/ns/json-ld#context\"");
    assert_eq!(
        check_link(&links(&[core.as_bytes(), b"\xe9"])),
        Err(REFUSAL)
    );
}

#[test]
fn a_link_without_parameters_names_no_context() {
    for value in [
        "<https://example.org/ctx.jsonld>",
        "https://example.org/ctx.jsonld",
        "",
        ",,",
        "<https://example.org/a>, <https://example.org/b>",
    ] {
        assert_eq!(check_link(&links(&[value.as_bytes()])), Ok(()), "{value:?}");
    }
    // One bare link beside a context link that is not the core one: refused for the second.
    let mixed = "<https://example.org/a>, <https://evil.example/ctx>; rel=\"http://www.w3.org/ns/json-ld#context\"";
    assert_eq!(check_link(&links(&[mixed.as_bytes()])), Err(REFUSAL));
    // A rel among several, in any case of `rel`, unquoted: still the context rel.
    let spaced = "<https://evil.example/ctx>; type=\"application/ld+json\"; REL = \"alternate http://www.w3.org/ns/json-ld#context\"";
    assert_eq!(check_link(&links(&[spaced.as_bytes()])), Err(REFUSAL));
    // Another rel, or a parameter without `=`, names no context.
    let other = "<https://evil.example/ctx>; rel=alternate; rel; =x";
    assert_eq!(check_link(&links(&[other.as_bytes()])), Ok(()));
    assert_eq!(check_link(&HeaderMap::new()), Ok(()));
}

#[test]
fn only_published_core_context_urls_are_core() {
    for url in [
        CORE,
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.10.jsonld",
    ] {
        assert!(is_core_context(url), "{url}");
    }
    for url in [
        "",
        "http://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context.jsonld",
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context",
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1..jsonld",
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.8a.jsonld",
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v2.1.jsonld",
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context.jsonld?x=1",
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.٣.jsonld",
    ] {
        assert!(!is_core_context(url), "{url}");
    }
}

#[test]
fn a_body_top_level_that_is_no_object_is_left_alone() {
    for body in [
        &b"[1, \"two\", null, true]"[..],
        b"7",
        b"\"@context\"",
        b"null",
        b"[]",
        b"[[{\"@context\": 1}]]",
    ] {
        // A nested array's object is not at an entity's level and is not looked into.
        assert_eq!(
            check_body(body),
            Ok(()),
            "{}",
            String::from_utf8_lossy(body)
        );
    }
    // Not JSON at all: left to the checks that refuse it.
    assert_eq!(check_body(b"{\"@context\": "), Ok(()));
    assert_eq!(check_body(b"\xff\xfe"), Ok(()));
    assert_eq!(check_body(b""), Ok(()));
}

#[test]
fn a_context_inside_a_list_or_deeper_is_refused() {
    for body in [
        json!({ "id": "x", "list": [1, { "@context": CORE }] }),
        json!({ "a": [[[{ "b": { "@context": [] } }]]] }),
        json!([{ "id": "x" }, { "nested": [{ "@context": "https://example.org" }] }]),
        json!({ "@context": [CORE, 1] }),
        json!({ "@context": [CORE, "https://example.org/c.jsonld"] }),
        json!({ "@context": { "pm10": "https://example.org/pm10" } }),
        json!({ "@context": null }),
    ] {
        assert_eq!(
            check_body(body.to_string().as_bytes()),
            Err(REFUSAL),
            "{body}"
        );
    }
    for body in [
        json!({ "@context": [] }),
        json!({ "@context": [CORE, CORE] }),
        json!([{ "@context": CORE }, { "list": [1, "two", [null]] }]),
    ] {
        assert_eq!(check_body(body.to_string().as_bytes()), Ok(()), "{body}");
    }
}

#[test]
fn a_very_deep_body_is_read_without_overflow() {
    // serde_json stops at its recursion limit, so a hostile depth is "not JSON", never a crash.
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert_eq!(check_body(deep.as_bytes()), Ok(()));
    let within = format!(
        "{{\"a\":{}{{\"@context\":1}}{}}}",
        "[".repeat(100),
        "]".repeat(100)
    );
    assert_eq!(check_body(within.as_bytes()), Err(REFUSAL));
}

fn schema() -> Value {
    json!({ "$defs": {
        "Station": { "properties": {
            "pm10": { "x-unit": { "exactMappings": ["ucefact:GQ"] } },
            "crates": { "x-unit": { "exactMappings": ["ucefact:10"] } },
            "odd": { "x-unit": { "exactMappings": ["ucefact:ZZZ"] } }
        } }
    } })
}

fn rules(missing: MissingUnitCode) -> UnitRules {
    UnitRules::from_schema(&schema(), missing)
}

fn whole() -> Shape<'static> {
    Shape {
        addressed: None,
        targeted: None,
    }
}

#[test]
fn a_schema_without_units_measures_nothing() {
    for schema in [
        json!(null),
        json!({ "definitions": 1 }),
        json!({ "definitions": { "A": { "type": "object" } } }),
        json!({ "definitions": { "A": { "properties": { "n": { "type": "string" } } } } }),
        json!({ "definitions": { "A": { "properties": {
            "a": { "x-unit": { "exactMappings": "ucefact:GQ" } },
            "b": { "x-unit": { "exactMappings": [1, "qudt-unit:DEG_C", "ucefact:"] } },
            "c": { "x-unit": {} },
            "d": { "x-unit": { "exactMappings": ["ucefact:GQ"] }, "x-ngsi-ld-kind": "GeoProperty" }
        } } } }),
    ] {
        let rules = UnitRules::from_schema(&schema, MissingUnitCode::Fill);
        assert!(rules.is_empty(), "{schema}");
    }
    // One class without units beside one with: only the second is held.
    let mixed = UnitRules::from_schema(
        &json!({ "definitions": {
            "Plain": { "properties": { "n": { "type": "string" } } },
            "Room": { "properties": { "t": { "x-unit": { "exactMappings": ["unece:CEL"] } } } }
        } }),
        MissingUnitCode::Refuse,
    );
    assert_eq!(mixed.by_class.len(), 1);
    assert_eq!(mixed.by_class["Room"]["t"], "CEL");
    assert_eq!(mixed.missing, MissingUnitCode::Refuse);
}

#[test]
fn an_empty_model_leaves_the_payload_untouched() {
    let rules = UnitRules::default();
    let mut payload = json!({ "type": "Station", "pm10": 3 });
    let before = payload.clone();
    assert_eq!(enforce(&mut payload, whole(), &rules), Ok(vec![]));
    assert_eq!(payload, before);
}

#[test]
fn each_refusal_names_the_unit_as_well_as_it_is_known() {
    let missing = |code: &str| UnitRefusal::Missing {
        class: "Station".into(),
        attribute: "a".into(),
        expected: code.into(),
    };
    let wrong = |code: &str| UnitRefusal::Wrong {
        class: "Station".into(),
        attribute: "a".into(),
        expected: code.into(),
        found: "KGM".into(),
    };
    let detail = |r: UnitRefusal| {
        let p = ProblemDetails::from(r);
        assert_eq!(p.status, 400);
        p.detail.expect("a detail")
    };
    // A code with a symbol, one without, and one Rec 20 does not list.
    assert!(detail(missing("GQ")).contains("(`GQ` (µg/m³, microgram per cubic metre))"));
    assert!(detail(missing("10")).contains("(`10` (group))"));
    assert!(detail(missing("ZZZ")).contains("(`ZZZ`)"));
    let text = detail(missing("GQ"));
    assert!(text.starts_with("attribute `a` of `Station` needs unitCode `GQ`: this space refuses a quantity without its unit"), "{text}");
    assert!(text.ends_with("(DM-06)"));
    let text = detail(wrong("10"));
    assert!(
        text.contains(
            "is measured in `10` (group) by the space's data model, and the write says `KGM`"
        ),
        "{text}"
    );
    assert!(detail(wrong("ZZZ")).contains("measured in `ZZZ` by"));
}

#[test]
fn every_value_form_is_judged_as_a_quantity_or_left_alone() {
    let fill = rules(MissingUnitCode::Fill);
    // Not a Property, an object that is neither form, null and NGSI-LD's null: untouched.
    for value in [
        json!({ "type": "Relationship", "object": "urn:ngsi-ld:X:1" }),
        json!({ "type": "GeoProperty", "value": { "type": "Point" } }),
        json!({ "object": "urn:ngsi-ld:X:1" }),
        json!({}),
        json!(null),
        json!("urn:ngsi-ld:null"),
        json!({ "type": "Property", "value": "urn:ngsi-ld:null" }),
        json!({ "value": 3, "unitCode": "GQ" }),
        json!([]),
    ] {
        let mut payload = json!({ "type": "Station", "pm10": value.clone() });
        assert_eq!(enforce(&mut payload, whole(), &fill), Ok(vec![]), "{value}");
        assert_eq!(payload["pm10"], value);
    }
    // A non-string unitCode is a wrong one, shown as JSON.
    let mut payload = json!({ "type": "Station", "pm10": { "value": 3, "unitCode": 7 } });
    match enforce(&mut payload, whole(), &fill) {
        Err(UnitRefusal::Wrong {
            found, expected, ..
        }) => assert_eq!((found.as_str(), expected.as_str()), ("7", "GQ")),
        other => panic!("{other:?}"),
    }
    // A wrong code in the second instance refuses the whole write.
    let mut payload =
        json!({ "type": "Station", "pm10": [{ "value": 1 }, { "value": 2, "unitCode": "KGM" }] });
    assert!(matches!(
        enforce(&mut payload, whole(), &fill),
        Err(UnitRefusal::Wrong { .. })
    ));
}

#[test]
fn a_batch_names_each_filled_quantity_once() {
    let fill = rules(MissingUnitCode::Fill);
    let mut batch = json!([
        { "type": "Station", "pm10": 1 },
        { "type": "Station", "pm10": { "value": 2 }, "crates": 4 },
        { "type": "Station", "pm10": [3, { "type": "Property", "value": 4 }] },
        7,
        { "type": "Other", "pm10": 5 }
    ]);
    let filled = enforce(&mut batch, whole(), &fill).expect("filled");
    assert_eq!(filled, vec!["pm10=GQ".to_owned(), "crates=10".to_owned()]);
    assert_eq!(batch[0]["pm10"], json!({ "value": 1, "unitCode": "GQ" }));
    assert_eq!(batch[1]["pm10"], json!({ "value": 2, "unitCode": "GQ" }));
    assert_eq!(batch[2]["pm10"][0], json!({ "value": 3, "unitCode": "GQ" }));
    assert_eq!(batch[2]["pm10"][1]["unitCode"], json!("GQ"));
    assert_eq!(batch[3], json!(7));
    assert_eq!(
        batch[4]["pm10"],
        json!(5),
        "a class the model does not measure"
    );
}

#[test]
fn a_space_that_refuses_refuses_every_form_without_a_code() {
    let refuse = rules(MissingUnitCode::Refuse);
    for value in [
        json!(3),
        json!("3"),
        json!(true),
        json!({ "value": 3 }),
        json!([{ "value": 3, "unitCode": "GQ" }, 4]),
    ] {
        let mut payload = json!({ "type": "Station", "pm10": value.clone() });
        match enforce(&mut payload, whole(), &refuse) {
            Err(UnitRefusal::Missing {
                class,
                attribute,
                expected,
            }) => {
                assert_eq!(
                    (class.as_str(), attribute.as_str(), expected.as_str()),
                    ("Station", "pm10", "GQ"),
                    "{value}"
                );
            }
            other => panic!("{value}: {other:?}"),
        }
    }
}

#[test]
fn a_fragment_for_one_attribute_keeps_its_value_even_when_refused() {
    let refuse = rules(MissingUnitCode::Refuse);
    let shape = Shape {
        addressed: Some("urn:ngsi-ld:Station:x:1"),
        targeted: Some("pm10"),
    };
    let mut bare = json!(12);
    assert!(enforce(&mut bare, shape, &refuse).is_err());
    assert_eq!(bare, json!(12), "the caller's payload is put back");
    let fill = rules(MissingUnitCode::Fill);
    assert_eq!(
        enforce(&mut bare, shape, &fill),
        Ok(vec!["pm10=GQ".to_owned()])
    );
    assert_eq!(bare, json!({ "value": 12, "unitCode": "GQ" }));
    // An address of no type: nothing to check.
    let unknown = Shape {
        addressed: Some("x"),
        targeted: Some("pm10"),
    };
    let mut bare = json!(12);
    assert_eq!(enforce(&mut bare, unknown, &refuse), Ok(vec![]));
    assert_eq!(bare, json!(12));
}
