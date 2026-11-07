// SPDX-License-Identifier: EUPL-1.2
//! The interpreter alone: the gateway's `view_mapping` cases ported
//! (joinedcontext-platform `context-gateway/tests/view_mapping_tests.rs`),
//! every cell of `view-mapping.md` table 4.2, and the request checks of
//! section 4.1.

use super::*;
use serde_json::json;

const STATION: &str = "urn:ngsi-ld:AirQualityObserved:senzor-01";

/// The IR Model Tools compiles: the local model on the left, Smart Data
/// Models on the right.
fn ir() -> Value {
    json!({
        "version": 2,
        "sourceClass": "MestskySenzor",
        "targetClass": "AirQualityObserved",
        "slots": [
            { "target": "pm25", "source": "pm2p5", "kind": "rename", "filterable": true },
            { "target": "temperature", "source": "teplota", "kind": "unitConversion",
              "factor": 1.0, "offset": -273.15, "filterable": true },
            { "target": "reliability", "source": "spolahlivost", "kind": "valueMappings",
              "forward": { "vysoka": "high", "nizka": "low" },
              "inverse": { "high": "vysoka", "low": "nizka" }, "filterable": true },
            { "target": "stationCount", "source": "pocet", "kind": "cast",
              "range": "integer", "filterable": true },
            { "target": "dataProvider", "kind": "constant", "value": "bb", "filterable": false },
            { "target": "label", "kind": "expr", "filterable": false,
              "expression": { "binary": "+",
                "left": { "binary": "+", "left": { "slot": "nazov" }, "right": { "const": " (" } },
                "right": { "binary": "+", "left": { "slot": "spolahlivost" }, "right": { "const": ")" } } } }
        ]
    })
}

fn compile(ir: &Value) -> Result<Mapping, String> {
    Mapping::compile(ir, &antares_jsonld::core_context(), None)
}

fn mapping() -> Mapping {
    compile(&ir()).unwrap_or_else(|e| unreachable!("the IR compiles: {e}"))
}

fn slots(slots: &Value) -> Mapping {
    compile(&json!({ "version": 2, "sourceClass": "S", "targetClass": "T", "slots": slots }))
        .unwrap_or_else(|e| unreachable!("the IR compiles: {e}"))
}

/// The generated term the broker's answer names a source Attribute by.
fn term(m: &Mapping, source: &str) -> String {
    let iri = antares_jsonld::core_context().expand_key(source);
    m.inner_context()
        .as_object()
        .and_then(|o| o.iter().find(|(_, v)| v.as_str() == Some(iri.as_str())))
        .map(|(k, _)| k.clone())
        .unwrap_or_default()
}

/// An Entity as the broker answers it under the inner `@context`: source
/// names as their generated terms.
fn answered(m: &Mapping, entity: &Value) -> Value {
    let mut out = Map::new();
    for (k, v) in entity.as_object().into_iter().flatten() {
        let t = term(m, k);
        let k = if t.is_empty() || ["id", "type"].contains(&k.as_str()) {
            k.clone()
        } else {
            t
        };
        out.insert(k, v.clone());
    }
    Value::Object(out)
}

/// The entity as the broker holds it, in the source model's own names.
fn stored() -> Value {
    json!({
        "id": STATION,
        "type": "jcSrcClass",
        "modifiedAt": "2026-09-07T08:00:00Z",
        "nazov": { "type": "Property", "value": "Senzor Fončorda" },
        "pm2p5": { "type": "Property", "value": 12.5 },
        "teplota": { "type": "Property", "value": 292.15 },
        "spolahlivost": { "type": "Property", "value": "vysoka" },
        "pocet": { "type": "Property", "value": "4" },
        "kalibracia": { "type": "Property", "value": 0.3 }
    })
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn invert_q(m: &Mapping, q: &str) -> Result<String, &'static str> {
    m.invert(&pairs(&[("q", q)]))
        .map(|i| i.pairs.first().map(|p| p.1.clone()).unwrap_or_default())
}

/// `q` written in the target model, with the generated terms the inner
/// request names source Attributes by put back to their source names, so a
/// case reads as the gateway's did.
fn sent(m: &Mapping, q: &str) -> Result<String, &'static str> {
    let mut out = invert_q(m, q)?;
    let inner = m.inner_context().as_object().cloned().unwrap_or_default();
    // longest term first, so jcSrc1 does not eat into jcSrc10
    let mut terms: Vec<_> = inner.iter().collect();
    terms.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    for (t, iri) in terms {
        let name = antares_jsonld::core_context().compact_iri(iri.as_str().unwrap_or_default());
        out = out.replace(t.as_str(), &name);
    }
    Ok(out)
}

#[test]
fn an_answer_comes_back_in_the_target_model() {
    let m = mapping();
    let mut entity = answered(&m, &stored());
    m.rebuild(&mut entity, &Keep::default());
    assert_eq!(
        entity["type"],
        json!("AirQualityObserved"),
        "the target class"
    );
    assert_eq!(entity["pm25"]["value"], json!(12.5), "a rename");
    assert_eq!(
        entity["temperature"]["value"],
        json!(19.0),
        "kelvin to celsius"
    );
    assert_eq!(
        entity["reliability"]["value"],
        json!("high"),
        "a value mapping"
    );
    assert_eq!(entity["stationCount"]["value"], json!(4), "a cast");
    assert_eq!(
        entity["dataProvider"],
        json!({ "type": "Property", "value": "bb" }),
        "a constant"
    );
    assert_eq!(
        entity["label"],
        json!({ "type": "Property", "value": "Senzor Fončorda (vysoka)" }),
        "a computed slot reads the source attributes"
    );
    assert_eq!(entity["id"], json!(STATION));
    assert_eq!(
        entity["modifiedAt"],
        json!("2026-09-07T08:00:00Z"),
        "system members stay"
    );
    let names: BTreeSet<&str> = entity
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "id",
            "type",
            "modifiedAt",
            "pm25",
            "temperature",
            "reliability",
            "stationCount",
            "dataProvider",
            "label"
        ]),
        "no source name, input or underived attribute survives: {entity}"
    );
}

#[test]
fn a_filter_written_in_the_target_model_reaches_the_broker_in_the_source_model() {
    let m = mapping();
    let inv = m
        .invert(&pairs(&[
            ("type", "AirQualityObserved"),
            ("attrs", "pm25,temperature"),
            ("q", "reliability==\"high\";temperature>19"),
        ]))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert!(!inv.selects_nothing);
    assert_eq!(
        inv.pairs.first(),
        Some(&("type".to_owned(), "jcSrcClass".to_owned()))
    );
    let attrs = format!("{},{}", term(&m, "pm2p5"), term(&m, "teplota"));
    assert_eq!(inv.pairs.get(1), Some(&("attrs".to_owned(), attrs)));
    assert_eq!(
        sent(&m, "reliability==\"high\";temperature>19"),
        Ok("spolahlivost==\"vysoka\",\"high\";teplota>292.15".to_owned())
    );
}

#[test]
fn a_filter_on_a_slot_the_mapping_cannot_invert_is_refused() {
    let m = mapping();
    for q in [
        "label==\"x\"",
        "dataProvider==\"bb\"",
        "label",
        "!dataProvider",
    ] {
        assert_eq!(invert_q(&m, q), Err(NOT_FILTERABLE), "{q}");
    }
    assert_eq!(
        invert_q(&m, "kalibracia>0"),
        Err(UNKNOWN),
        "a source name is not smuggled through"
    );
    assert_eq!(
        invert_q(&m, &format!("{}>0", term(&m, "pm2p5"))),
        Err(UNKNOWN),
        "nor a generated term"
    );
}

#[test]
fn the_two_directions_agree_on_every_derivation() {
    let m = mapping();
    for (target, produced, source, stored) in [
        (
            "reliability",
            "\"high\"",
            "spolahlivost",
            "\"vysoka\",\"high\"",
        ),
        ("temperature", "19", "teplota", "292.15"),
        ("pm25", "12.5", "pm2p5", "12.5"),
    ] {
        assert_eq!(
            sent(&m, &format!("{target}=={produced}")),
            Ok(format!("{source}=={stored}"))
        );
    }
}

#[test]
fn an_ir_this_broker_does_not_run_is_refused_at_load() {
    let with = |key: &str, v: Value| {
        let mut d = ir();
        d[key] = v;
        compile(&d).err().unwrap_or_default()
    };
    assert!(with("version", json!(3)).contains("version 3"));
    assert!(with("version", json!("2")).contains("no version"));
    assert!(with("sourceClass", json!(7)).contains("sourceClass is a name"));
    assert!(with("slots", json!({})).contains("lists its slots"));
    assert!(with("slots", json!([])).contains("1 to 256"));
    let many: Vec<Value> = (0..=MAX_SLOTS)
        .map(|i| json!({ "target": format!("t{i}"), "source": "s", "kind": "rename" }))
        .collect();
    assert!(with("slots", Value::Array(many)).contains("1 to 256"));
    assert!(compile(&json!([]))
        .err()
        .unwrap_or_default()
        .contains("an object"));
    let mut ctx = antares_jsonld::core_context();
    let bad = json!({ "Bad": "@id" });
    ctx.merge_object(bad.as_object().unwrap_or(&Map::new()))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let mut d = ir();
    d["targetClass"] = json!("Bad");
    let e = Mapping::compile(&d, &ctx, None).err().unwrap_or_default();
    assert!(e.contains("does not expand"), "{e}");
}

#[test]
fn an_unknown_ir_member_is_refused_by_name() {
    let got = compile(
        &json!({ "version": 2, "sourceClass": "A", "targetClass": "B", "unitCode": "CEL",
        "slots": [{ "target": "x", "source": "y", "kind": "rename" }] }),
    );
    assert_eq!(
        got.err().as_deref(),
        Some("mapping: the IR member \"unitCode\" is not one of version 2")
    );
}

#[test]
fn a_malformed_slot_is_refused_with_its_index() {
    let refused = |slot: Value| {
        compile(&json!({ "version": 2, "sourceClass": "A", "targetClass": "B", "slots": [slot] }))
            .err()
            .unwrap_or_default()
    };
    for (slot, says) in [
        (json!("x"), "slot 0: not an object"),
        (
            json!({ "source": "y", "kind": "rename" }),
            "slot 0 target is a name",
        ),
        (
            json!({ "target": "x", "source": 1, "kind": "rename" }),
            "slot 0 source is a name",
        ),
        (json!({ "target": "x", "source": "y" }), "names no kind"),
        // T-5207: a misspelt or newer member refuses the IR instead of reading as absent
        (
            json!({ "target": "x", "source": "y", "kind": "unitConversion", "factor": 2, "ofset": 1 }),
            "member \"ofset\" is not one a unitConversion slot takes",
        ),
        (
            json!({ "target": "x", "source": "y", "kind": "rename", "factor": 2 }),
            "member \"factor\" is not one a rename slot takes",
        ),
        (
            json!({ "target": "x", "source": "y", "kind": "join" }),
            "\"join\" is not one",
        ),
        (
            json!({ "target": "x", "kind": "rename" }),
            "names no source",
        ),
        (
            json!({ "target": "x", "source": "y", "kind": "rename", "filterable": "yes" }),
            "filterable is a boolean",
        ),
        (
            json!({ "target": "x", "source": "y", "kind": "unitConversion" }),
            "carries a factor",
        ),
        (
            json!({ "target": "x", "source": "y", "kind": "unitConversion", "factor": 0 }),
            "non-zero",
        ),
        (
            json!({ "target": "x", "source": "y", "kind": "unitConversion", "factor": 1, "offset": "1" }),
            "an offset is a number",
        ),
        (
            json!({ "target": "x", "source": "y", "kind": "valueMappings" }),
            "forward table",
        ),
        (
            json!({ "target": "x", "source": "y", "kind": "cast", "range": "date" }),
            "integer, float or string",
        ),
        (
            json!({ "target": "x", "kind": "constant" }),
            "carries a value",
        ),
        (
            json!({ "target": "x", "kind": "expr" }),
            "carries no expression",
        ),
    ] {
        let e = refused(slot.clone());
        assert!(e.contains(says), "{slot}: {e}");
    }
    let twice = compile(
        &json!({ "version": 2, "sourceClass": "A", "targetClass": "B", "slots": [
        { "target": "x", "source": "y", "kind": "rename" },
        { "target": "x", "source": "z", "kind": "rename" }
    ] }),
    );
    assert!(twice.err().unwrap_or_default().contains("two slots serve"));
}

#[test]
fn a_slot_that_does_not_say_it_is_filterable_is_not() {
    let m = slots(&json!([{ "target": "x", "source": "y", "kind": "rename" }]));
    assert_eq!(invert_q(&m, "x==1"), Err(NOT_FILTERABLE));
    assert_eq!(
        m.source_attribute("x").map(|(i, _)| i),
        Ok(0),
        "it is still served"
    );
}

#[test]
fn attrs_asks_the_broker_for_what_a_computed_slot_reads_and_the_answer_keeps_only_what_was_asked() {
    let m = mapping();
    let inv = m
        .invert(&pairs(&[("attrs", "label,pm25")]))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let asked = format!(
        "{},{},{}",
        term(&m, "nazov"),
        term(&m, "spolahlivost"),
        term(&m, "pm2p5")
    );
    assert_eq!(
        inv.pairs,
        [("attrs".to_owned(), asked)],
        "the inputs of the expression, then the rename"
    );
    let mut entity = answered(&m, &stored());
    m.rebuild(&mut entity, &inv.keep);
    assert!(entity.get("label").is_some() && entity.get("pm25").is_some());
    assert!(
        entity.get("reliability").is_none(),
        "an input fetched for an expression is no attribute the caller asked for: {entity}"
    );
    let only_constant = m
        .invert(&pairs(&[("attrs", "dataProvider")]))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert!(
        only_constant.pairs.is_empty(),
        "nothing to ask the broker for"
    );
    let mut entity = answered(&m, &stored());
    m.rebuild(&mut entity, &only_constant.keep);
    assert_eq!(entity["dataProvider"]["value"], json!("bb"));
    assert!(entity.get("pm25").is_none());
    assert_eq!(
        m.invert(&pairs(&[("attrs", "pm25,nope")])).err(),
        Some(UNKNOWN)
    );
    assert_eq!(
        m.invert(&pairs(&[("attrs", ",")])).err(),
        Some(UNKNOWN),
        "attrs names at least one"
    );
}

fn computed(slots: &Value) -> Mapping {
    compile(&json!({ "version": 2, "sourceClass": "A", "targetClass": "B", "slots": slots }))
        .unwrap_or_else(|e| unreachable!("{e}"))
}

fn slot_of(target: &str, expression: &Value) -> Value {
    json!({ "target": target, "kind": "expr", "filterable": false, "expression": expression })
}

fn rebuilt(m: &Mapping, entity: &Value) -> Value {
    let mut e = answered(m, entity);
    m.rebuild(&mut e, &Keep::default());
    e
}

/// Each computed slot of `cases` over one Entity holds its expected value.
fn evaluates(cases: &[(&str, Value, Value)]) {
    let listed: Vec<Value> = cases.iter().map(|(n, e, _)| slot_of(n, e)).collect();
    let m = computed(&Value::Array(listed));
    let e = rebuilt(
        &m,
        &json!({
            "id": "urn:ngsi-ld:A:1", "type": "jcSrcClass",
            "a": { "type": "Property", "value": 4 },
            "b": { "type": "Property", "value": 3 },
            "f": { "type": "Property", "value": 1.5 },
            "on": { "type": "Property", "value": true },
            "name": { "type": "Property", "value": "sensor" }
        }),
    );
    for (slot, _, value) in cases {
        assert_eq!(&e[*slot]["value"], value, "{slot}: {e}");
    }
}

fn op(kind: &str, op: &str, left: &Value, right: &Value) -> Value {
    json!({ kind: op, "left": left, "right": right })
}

#[test]
fn every_arithmetic_form_evaluates() {
    let (a, b, f) = (
        json!({ "slot": "a" }),
        json!({ "slot": "b" }),
        json!({ "slot": "f" }),
    );
    let k = |v: Value| json!({ "const": v });
    evaluates(&[
        ("sum", op("binary", "+", &a, &b), json!(7)),
        ("diff", op("binary", "-", &a, &b), json!(1)),
        ("prod", op("binary", "*", &a, &b), json!(12)),
        // a division answers a fraction, as Python and Bloblang do
        ("half", op("binary", "/", &a, &k(json!(2))), json!(2.0)),
        ("fsum", op("binary", "+", &f, &k(json!(1))), json!(2.5)),
        ("fdiff", op("binary", "-", &f, &k(json!(1))), json!(0.5)),
        ("fprod", op("binary", "*", &f, &k(json!(2))), json!(3.0)),
        ("negated", json!({ "unary": "-", "operand": b }), json!(-3)),
        ("fneg", json!({ "unary": "-", "operand": f }), json!(-1.5)),
        (
            "joined",
            op("binary", "+", &json!({ "slot": "name" }), &k(json!("!"))),
            json!("sensor!"),
        ),
    ]);
}

#[test]
fn every_comparison_and_junction_evaluates() {
    let (a, b) = (json!({ "slot": "a" }), json!({ "slot": "b" }));
    let (name, on) = (json!({ "slot": "name" }), json!({ "slot": "on" }));
    let k = |v: Value| json!({ "const": v });
    evaluates(&[
        ("over", op("compare", ">", &a, &k(json!(3))), json!(true)),
        ("ne", op("compare", "!=", &a, &k(json!(3))), json!(true)),
        ("le", op("compare", "<=", &a, &k(json!(4))), json!(true)),
        (
            "ge",
            op("compare", ">=", &name, &k(json!("s"))),
            json!(true),
        ),
        (
            "eqb",
            op("compare", "==", &on, &k(json!(true))),
            json!(true),
        ),
        (
            "neb",
            op("compare", "!=", &on, &k(json!(true))),
            json!(false),
        ),
        (
            "both",
            json!({ "boolean": "and", "operands": [
                op("compare", ">=", &a, &k(json!(4))),
                { "unary": "not", "operand": op("compare", "==", &name, &k(json!("x"))) }
            ] }),
            json!(true),
        ),
        (
            "either",
            json!({ "boolean": "or", "operands": [
                op("compare", "<", &a, &k(json!(0))),
                op("compare", "!=", &b, &k(json!(3)))
            ] }),
            json!(false),
        ),
    ]);
}

#[test]
fn a_computed_slot_with_nothing_to_compute_from_is_left_out() {
    let m = computed(&json!([
        slot_of(
            "missing",
            &json!({ "binary": "+", "left": { "slot": "absent" }, "right": { "const": 1 } })
        ),
        slot_of(
            "mixed",
            &json!({ "binary": "+", "left": { "slot": "name" }, "right": { "slot": "a" } })
        ),
        slot_of(
            "strings",
            &json!({ "binary": "-", "left": { "slot": "name" }, "right": { "const": "x" } })
        ),
        slot_of(
            "incomparable",
            &json!({ "compare": "<", "left": { "slot": "name" }, "right": { "slot": "a" } })
        ),
        slot_of(
            "ordered",
            &json!({ "compare": "<", "left": { "slot": "on" }, "right": { "const": false } })
        ),
        slot_of(
            "divided",
            &json!({ "binary": "/", "left": { "slot": "a" }, "right": { "const": 0 } })
        ),
        slot_of(
            "overflow",
            &json!({ "binary": "*", "left": { "slot": "big" }, "right": { "const": 2 } })
        ),
        slot_of(
            "infinite",
            &json!({ "binary": "*", "left": { "const": 1e308 }, "right": { "const": 1e308 } })
        ),
        slot_of(
            "minimum",
            &json!({ "unary": "-", "operand": { "slot": "min" } })
        ),
        slot_of(
            "not_a_number",
            &json!({ "unary": "-", "operand": { "slot": "name" } })
        ),
        slot_of(
            "not_a_bool",
            &json!({ "unary": "not", "operand": { "slot": "a" } })
        ),
        slot_of(
            "junction",
            &json!({ "boolean": "or", "operands": [{ "const": true }, { "slot": "a" }] })
        ),
        slot_of(
            "present",
            &json!({ "binary": "*", "left": { "slot": "a" }, "right": { "const": 10 } })
        )
    ]));
    let e = rebuilt(
        &m,
        &json!({
            "id": "urn:ngsi-ld:A:1", "type": "jcSrcClass",
            "a": { "type": "Property", "value": 4 },
            "on": { "type": "Property", "value": true },
            "big": { "type": "Property", "value": i64::MAX },
            "min": { "type": "Property", "value": i64::MIN },
            "name": { "type": "Property", "value": "sensor" }
        }),
    );
    for absent in [
        "missing",
        "mixed",
        "strings",
        "incomparable",
        "ordered",
        "divided",
        "overflow",
        "infinite",
        "minimum",
        "not_a_number",
        "not_a_bool",
        "junction",
    ] {
        assert!(
            e.get(absent).is_none(),
            "{absent} has no value, and null is not one: {e}"
        );
    }
    assert_eq!(
        e["present"]["value"],
        json!(40),
        "the rest of the view still answers"
    );
}

#[test]
fn an_expression_reads_a_key_value_entity_as_readily_as_a_normalized_one() {
    let m = computed(&json!([slot_of(
        "joined",
        &json!({ "binary": "+", "left": { "slot": "name" }, "right": { "slot": "rel" } })
    )]));
    let e = rebuilt(
        &m,
        &json!({ "id": "urn:ngsi-ld:A:1", "type": "jcSrcClass", "name": "a", "rel": "b" }),
    );
    assert_eq!(e["joined"], json!("ab"));
    let e = rebuilt(
        &m,
        &json!({ "id": "urn:ngsi-ld:A:1", "type": "jcSrcClass",
        "name": { "type": "Property", "value": "a" }, "rel": { "type": "Relationship", "object": "b" } }),
    );
    assert_eq!(e["joined"], json!({ "type": "Property", "value": "ab" }));
}

#[test]
fn an_expression_node_this_broker_does_not_evaluate_is_refused() {
    for node in [
        json!({ "call": "upper", "operand": { "slot": "a" } }),
        json!({ "binary": "**", "left": { "slot": "a" }, "right": { "const": 2 } }),
        json!({ "binary": "+", "left": { "slot": "a" } }),
        json!({ "compare": "<>", "left": { "slot": "a" }, "right": { "const": 2 } }),
        json!({ "boolean": "xor", "operands": [{ "const": true }] }),
        json!({ "boolean": "and", "operands": [] }),
        json!({ "boolean": "and" }),
        json!({ "boolean": "and", "operands": [{ "nope": 1 }] }),
        json!({ "unary": "~", "operand": { "slot": "a" } }),
        json!({ "unary": "-" }),
        json!({ "slot": 1 }),
        json!("a"),
    ] {
        let refused = compile(
            &json!({ "version": 2, "sourceClass": "A", "targetClass": "B",
            "slots": [slot_of("label", &node)] }),
        );
        assert!(
            refused.err().unwrap_or_default().contains("slot 0"),
            "{node} was accepted"
        );
    }
}

#[test]
fn a_computed_slot_is_refused_in_a_filter_an_order_and_a_geoquery() {
    let m = computed(&json!([slot_of(
        "label",
        &json!({ "binary": "+", "left": { "slot": "name" }, "right": { "const": "!" } })
    )]));
    assert_eq!(
        invert_q(&m, "label==\"x!\""),
        Err(UNKNOWN).or(Err(NOT_FILTERABLE))
    );
    assert_eq!(
        m.invert(&pairs(&[("geoproperty", "label")])).err(),
        Some(NOT_FILTERABLE)
    );
    assert_eq!(
        m.invert(&pairs(&[("orderBy", "label")])).err(),
        Some(NOT_FILTERABLE)
    );
    assert_eq!(m.source_attribute("label").err(), Some(COMPUTED));
}

#[test]
fn a_separator_inside_a_quoted_value_stays_in_the_value() {
    let m = slots(
        &json!([{ "target": "name", "source": "nazov", "kind": "rename", "filterable": true }]),
    );
    assert_eq!(
        sent(&m, r#"name=="a;b|(c)""#),
        Ok(r#"nazov=="a;b|(c)""#.to_owned())
    );
}

#[test]
fn a_negative_factor_turns_the_comparison_around() {
    let m = slots(
        &json!([{ "target": "depth", "source": "height", "kind": "unitConversion",
        "factor": -1.0, "offset": 0.0, "filterable": true }]),
    );
    for (asked, sent_q) in [
        ("depth>5", "height<-5"),
        ("depth>=5", "height<=-5"),
        ("depth<5", "height>-5"),
        ("depth<=5", "height>=-5"),
        ("depth==1..3", "height==-3..-1"),
        ("depth!=1,2", "height!=-1,-2"),
        ("depth==2", "height==-2"),
    ] {
        assert_eq!(sent(&m, asked), Ok(sent_q.to_owned()), "{asked}");
    }
}

#[test]
fn a_positive_factor_keeps_the_order_and_inverts_the_values() {
    let m = slots(
        &json!([{ "target": "t", "source": "s", "kind": "unitConversion",
        "factor": 2.0, "offset": 1.0, "filterable": true }]),
    );
    for (asked, sent_q) in [
        ("t>5", "s>2"),
        ("t<=5", "s<=2"),
        ("t==1..5", "s==0..2"),
        ("t==3,5", "s==1,2"),
        ("t", "s"),
        ("!t", "!s"),
    ] {
        assert_eq!(sent(&m, asked), Ok(sent_q.to_owned()), "{asked}");
    }
    for refused in [
        "t==\"x\"",
        "t>\"2026-01-01\"",
        "t~=\"a.*\"",
        "t!~=\"a\"",
        "t==1,\"x\"",
        "t==\"a\"..\"b\"",
        "t.sub>1",
        "t[x]==1",
    ] {
        assert!(invert_q(&m, refused).is_err(), "{refused}");
    }
}

#[test]
fn a_many_to_one_table_inverts_to_every_source_value() {
    let m = slots(
        &json!([{ "target": "level", "source": "uroven", "kind": "valueMappings",
        "forward": { "a": "high", "b": "high", "c": "low", "high": "low", "7": "seven", "true": "yes" },
        "inverse": { "high": "a", "low": "c" }, "filterable": true }]),
    );
    for (asked, sent_q) in [
        ("level==\"high\"", r#"uroven=="a","b""#),
        ("level!=\"high\"", r#"uroven!="a","b""#),
        // `low` is no key, so a stored `low` passes unchanged and matches
        ("level==\"low\"", r#"uroven=="c","high","low""#),
        (
            "level==\"high\",\"low\"",
            r#"uroven=="a","b","c","high","low""#,
        ),
        // outside the table: the stored value as written
        ("level==\"none\"", r#"uroven=="none""#),
        ("level==seven", r#"uroven==7,"seven""#),
        ("level==\"yes\"", r#"uroven==true,"yes""#),
        ("level", "uroven"),
    ] {
        assert_eq!(sent(&m, asked), Ok(sent_q.to_owned()), "{asked}");
    }
    // `a` is a key mapped elsewhere and nothing maps to it: no entity shows it
    assert_eq!(sent(&m, "level==\"a\""), Ok("uroven;!uroven".to_owned()));
    assert_eq!(sent(&m, "level!=\"a\""), Ok("uroven".to_owned()));
    for refused in [
        "level>\"high\"",
        "level~=\"h.*\"",
        "level==\"a\"..\"b\"",
        "level.x==\"a\"",
    ] {
        assert!(invert_q(&m, refused).is_err(), "{refused}");
    }
}

#[test]
fn a_cast_a_constant_and_an_expression_compare_nowhere() {
    let m = slots(&json!([
        { "target": "n", "source": "s", "kind": "cast", "range": "integer", "filterable": true },
        { "target": "k", "kind": "constant", "value": 1, "filterable": true },
        { "target": "e", "kind": "expr", "filterable": true, "expression": { "slot": "s" } }
    ]));
    assert_eq!(
        sent(&m, "n"),
        Ok("s".to_owned()),
        "a cast slot exists where its source does"
    );
    for op in ["==3", "!=3", "<3", "<=3", ">3", ">=3", "~=\"3\""] {
        assert_eq!(
            invert_q(&m, &format!("n{op}")),
            Err(NOT_COMPARABLE),
            "n{op}"
        );
        for produced in ["k", "e"] {
            assert_eq!(
                invert_q(&m, &format!("{produced}{op}")),
                Err(NOT_FILTERABLE),
                "{produced}{op}"
            );
        }
    }
    for produced in ["k", "e", "!k"] {
        assert_eq!(invert_q(&m, produced), Err(NOT_FILTERABLE), "{produced}");
    }
}

#[test]
fn a_renamed_slot_keeps_every_operator_and_a_member_path() {
    let m = slots(
        &json!([{ "target": "name", "source": "nazov", "kind": "rename", "filterable": true }]),
    );
    for (asked, sent_q) in [
        ("name~=\"a.*\"", "nazov~=\"a.*\""),
        ("name!~=\"a\"", "nazov!~=\"a\""),
        ("name.first==\"a\"", "nazov.first==\"a\""),
        ("name[first]==\"a\"", "nazov[first]==\"a\""),
        (
            "name==\"a\"|(name>\"b\";!name)",
            "nazov==\"a\"|nazov>\"b\";!nazov",
        ),
    ] {
        assert_eq!(sent(&m, asked), Ok(sent_q.to_owned()), "{asked}");
    }
}

#[test]
fn a_linked_path_and_an_unparsable_q_are_refused() {
    let m = slots(
        &json!([{ "target": "name", "source": "nazov", "kind": "rename", "filterable": true }]),
    );
    assert_eq!(invert_q(&m, "name{name}==\"a\""), Err(DEEP_PATH));
    assert_eq!(invert_q(&m, "name=="), Err(BAD_Q));
}

#[test]
fn a_type_selects_the_source_class_or_nothing() {
    let m = mapping();
    let ty = |sel: &str| {
        m.invert(&pairs(&[("type", sel)]))
            .unwrap_or_else(|e| unreachable!("{e}"))
    };
    for served in [
        "AirQualityObserved",
        "*",
        "Other,AirQualityObserved",
        "Other|(AirQualityObserved)",
    ] {
        let i = ty(served);
        assert!(!i.selects_nothing, "{served}");
        assert_eq!(
            i.pairs,
            [("type".to_owned(), "jcSrcClass".to_owned())],
            "{served}"
        );
    }
    for nothing in ["Other", "MestskySenzor", "(AirQualityObserved;Other)"] {
        let i = ty(nothing);
        assert!(i.selects_nothing && i.pairs.is_empty(), "{nothing}");
    }
}

#[test]
fn pick_and_omit_apply_to_the_rebuilt_answer() {
    let m = mapping();
    let inv = m
        .invert(&pairs(&[("pick", "id,pm25,label"), ("omit", "label")]))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert!(inv.pairs.is_empty(), "neither goes to the broker");
    let mut e = answered(&m, &stored());
    e["@context"] = json!({ "jcSrc0": "x" });
    m.rebuild(&mut e, &inv.keep);
    assert_eq!(
        e,
        json!({ "@context": antares_jsonld::CORE_CONTEXT, "id": STATION,
                "pm25": { "type": "Property", "value": 12.5 } }),
        "the answer's @context is the view's, never the inner one"
    );
    for listed in ["type,modifiedAt,scope", "createdAt"] {
        assert!(m.invert(&pairs(&[("pick", listed)])).is_ok(), "{listed}");
    }
    assert_eq!(m.invert(&pairs(&[("omit", "pm2p5")])).err(), Some(UNKNOWN));
}

#[test]
fn order_by_follows_the_slot_kind() {
    let m = slots(&json!([
        { "target": "name", "source": "nazov", "kind": "rename", "filterable": true },
        { "target": "up", "source": "u", "kind": "unitConversion", "factor": 2, "filterable": true },
        { "target": "down", "source": "d", "kind": "unitConversion", "factor": -2, "filterable": true },
        { "target": "level", "source": "l", "kind": "valueMappings", "forward": {}, "filterable": true },
        { "target": "hidden", "source": "h", "kind": "rename" }
    ]));
    let order = |spec: &str| {
        m.invert(&pairs(&[("orderBy", spec)]))
            .map(|i| sent_names(&m, &i.pairs[0].1))
    };
    assert_eq!(
        order("name;dist-asc,up;desc"),
        Ok("nazov;dist-asc,u;desc".to_owned())
    );
    assert_eq!(order("up"), Ok("u".to_owned()));
    assert_eq!(order("down"), Ok("d;desc".to_owned()));
    assert_eq!(order("down;desc"), Ok("d;asc".to_owned()));
    assert_eq!(order("down;dist-asc").err(), Some(NOT_COMPARABLE));
    assert_eq!(order("up;dist-desc").err(), Some(NOT_COMPARABLE));
    assert_eq!(order("level").err(), Some(NOT_COMPARABLE));
    assert_eq!(order("hidden").err(), Some(NOT_FILTERABLE));
    assert_eq!(order("name.x").err(), Some(DEEP_PATH));
    assert_eq!(order("name[x]").err(), Some(DEEP_PATH));
    assert_eq!(order("nope").err(), Some(UNKNOWN));
}

fn sent_names(m: &Mapping, text: &str) -> String {
    let mut out = text.to_owned();
    for (t, iri) in m.inner_context().as_object().into_iter().flatten() {
        let name = antares_jsonld::core_context().compact_iri(iri.as_str().unwrap_or_default());
        out = out.replace(t.as_str(), &name);
    }
    out
}

#[test]
fn a_geoquery_names_a_renamed_filterable_slot() {
    let m = slots(&json!([
        { "target": "where", "source": "location", "kind": "rename", "filterable": true },
        { "target": "area", "source": "a", "kind": "rename" },
        { "target": "level", "source": "l", "kind": "valueMappings", "forward": {}, "filterable": true }
    ]));
    for name in ["geoproperty", "geometryProperty"] {
        let i = m
            .invert(&pairs(&[(name, "where")]))
            .unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(sent_names(&m, &i.pairs[0].1), "location", "{name}");
    }
    assert_eq!(
        m.invert(&pairs(&[("geoproperty", "area")])).err(),
        Some(NOT_FILTERABLE)
    );
    assert_eq!(
        m.invert(&pairs(&[("geoproperty", "level")])).err(),
        Some(NOT_FILTERABLE)
    );
    assert_eq!(
        m.invert(&pairs(&[("geoproperty", "nope")])).err(),
        Some(UNKNOWN)
    );
}

#[test]
fn a_parameter_is_passed_rewritten_or_refused() {
    let m = mapping();
    // `georel` alone adds the target's `location` (its own test)
    let passed: Vec<(&str, &str)> = PASSED
        .iter()
        .filter(|p| **p != "georel")
        .map(|p| (*p, "x"))
        .collect();
    let inv = m
        .invert(&pairs(&passed))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(
        inv.pairs,
        pairs(&passed),
        "these name no attribute and pass as written"
    );
    for format in ["normalized", "concise", "keyValues", "simplified"] {
        assert!(m.invert(&pairs(&[("format", format)])).is_ok(), "{format}");
    }
    for (name, value) in [
        ("format", "geojson"),
        ("lang", "en"),
        ("datasetId", "urn:x"),
        ("expandValues", "a"),
        ("join", "flat"),
        ("joinLevel", "1"),
        ("containedBy", "x"),
        ("jsonKeys", "a"),
        ("entityMap", "true"),
        ("splitEntities", "true"),
        ("entityMapLifetime", "1"),
        ("nope", "1"),
    ] {
        assert_eq!(
            m.invert(&pairs(&[(name, value)])).err(),
            Some(NOT_SERVED),
            "{name}"
        );
    }
    assert_eq!(
        m.invert(&pairs(&[("limit", "1"), ("limit", "2")])).err(),
        Some(TWICE)
    );
}

#[test]
fn retrieve_attribute_value_reads_a_source_attribute_and_rebuilds_it() {
    let m = mapping();
    let (i, source) = m
        .source_attribute("temperature")
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(source, term(&m, "teplota"));
    let mut bare = json!(292.15);
    assert!(m.rebuild_attribute(i, &mut bare));
    assert_eq!(bare, json!(19.0));
    let (i, _) = m
        .source_attribute("reliab%69lity")
        .unwrap_or_else(|e| unreachable!("{e}"));
    let mut attribute =
        json!({ "@context": { "jcSrc0": "x" }, "type": "Property", "value": "nizka" });
    assert!(m.rebuild_attribute(i, &mut attribute));
    assert_eq!(
        attribute,
        json!({ "@context": antares_jsonld::CORE_CONTEXT, "type": "Property", "value": "low" })
    );
    let mut plain = json!({ "type": "Property", "value": "nizka" });
    assert!(m.rebuild_attribute(i, &mut plain));
    assert_eq!(
        plain,
        json!({ "type": "Property", "value": "low" }),
        "a JSON answer gets no @context"
    );
    let mut untouched = json!({ "type": "Property", "value": 1 });
    assert!(!m.rebuild_attribute(99, &mut untouched));
    assert_eq!(
        untouched,
        json!({ "type": "Property", "value": 1 }),
        "no such slot"
    );
    assert_eq!(m.source_attribute("dataProvider").err(), Some(COMPUTED));
    assert_eq!(m.source_attribute("nope").err(), Some(UNKNOWN));
}

#[test]
fn every_representation_and_a_multi_attribute_are_rebuilt() {
    let m = slots(&json!([
        { "target": "t", "source": "s", "kind": "unitConversion", "factor": 2, "filterable": true },
        { "target": "r", "source": "o", "kind": "valueMappings", "forward": { "urn:a": "urn:b" } },
        { "target": "g", "source": "location", "kind": "unitConversion", "factor": 2 }
    ]));
    let point = json!({ "type": "Point", "coordinates": [1, 2] });
    let normalized = rebuilt(
        &m,
        &json!({ "id": "urn:x", "type": "jcSrcClass",
        "s": [{ "type": "Property", "value": 1, "datasetId": "urn:d1" }, { "type": "Property", "value": 2 }],
        "o": { "type": "Relationship", "object": "urn:a" },
        "location": { "type": "GeoProperty", "value": point } }),
    );
    assert_eq!(
        normalized["t"],
        json!([{ "type": "Property", "value": 2.0, "datasetId": "urn:d1" },
                                       { "type": "Property", "value": 4.0 }])
    );
    assert_eq!(
        normalized["r"],
        json!({ "type": "Relationship", "object": "urn:b" })
    );
    assert_eq!(
        normalized["g"]["value"], point,
        "a geometry has no scalar to convert"
    );
    let concise = rebuilt(
        &m,
        &json!({ "id": "urn:x", "type": "jcSrcClass",
        "s": { "value": 3 }, "o": { "object": "urn:a" } }),
    );
    assert_eq!(concise["t"], json!({ "value": 6.0 }));
    let simplified = rebuilt(
        &m,
        &json!({ "id": "urn:x", "type": "jcSrcClass",
        "s": 4, "o": "urn:a", "location": point.clone() }),
    );
    assert_eq!(simplified["t"], json!(8.0));
    assert_eq!(simplified["r"], json!("urn:b"));
    assert_eq!(simplified["g"], point);
    let not_a_number = rebuilt(
        &m,
        &json!({ "id": "urn:x", "type": "jcSrcClass", "s": "four", "o": ["urn:a"] }),
    );
    assert_eq!(
        not_a_number.get("t"),
        None,
        "a text is no quantity: not served in the source unit"
    );
    assert_eq!(
        not_a_number["r"],
        json!(["urn:b"]),
        "a simplified list is translated element by element"
    );
}

#[test]
fn a_conversion_that_leaves_json_numbers_keeps_the_value() {
    let m = slots(
        &json!([{ "target": "t", "source": "s", "kind": "unitConversion", "factor": 1e308 }]),
    );
    assert_eq!(
        rebuilt(&m, &json!({ "id": "urn:x", "s": 1e308 }))["t"],
        json!(1e308)
    );
}

#[test]
fn a_cast_follows_the_reference_engine() {
    let m = slots(&json!([
        { "target": "i", "source": "a", "kind": "cast", "range": "integer" },
        { "target": "f", "source": "b", "kind": "cast", "range": "float" },
        { "target": "s", "source": "c", "kind": "cast", "range": "string" }
    ]));
    let cast = |a: Value, b: Value, c: Value| {
        let e = rebuilt(&m, &json!({ "id": "urn:x", "a": a, "b": b, "c": c }));
        (e["i"].clone(), e["f"].clone(), e["s"].clone())
    };
    assert_eq!(
        cast(json!(-4.7), json!("2.5"), json!(3)),
        (json!(-4), json!(2.5), json!("3"))
    );
    assert_eq!(
        cast(json!("12"), json!(1), json!(true)),
        (json!(12), json!(1.0), json!("true"))
    );
    assert_eq!(
        cast(json!(7), json!("x"), json!("y")),
        (json!(7), json!("x"), json!("y"))
    );
    assert_eq!(
        cast(json!("4.5"), json!("NaN"), json!(null)),
        (json!("4.5"), json!("NaN"), json!("null"))
    );
    assert_eq!(
        cast(json!(1e20), json!(0), json!(0)).0,
        json!(1e20),
        "out of range stays as it is"
    );
}

#[test]
fn what_is_not_an_entity_passes_and_an_entity_without_attributes_is_normalized() {
    let m = slots(&json!([{ "target": "k", "kind": "constant", "value": 1 }]));
    for body in [
        json!({ "type": "https://uri.etsi.org/ngsi-ld/errors/NotFound" }),
        json!("x"),
        json!([1, "y"]),
    ] {
        let mut b = body.clone();
        m.rebuild(&mut b, &Keep::default());
        assert_eq!(b, body);
    }
    let mut page = json!([{ "@id": "urn:x", "type": "jcSrcClass" }]);
    m.rebuild(&mut page, &Keep::default());
    assert_eq!(
        page,
        json!([{ "@id": "urn:x", "type": "T", "k": { "type": "Property", "value": 1 } }])
    );
    let simplified = rebuilt(&m, &json!({ "id": "urn:x", "type": "jcSrcClass", "x": 2 }));
    assert_eq!(simplified["k"], json!(1));
}

#[test]
fn names_are_iris_through_the_views_own_context() {
    let mut ctx = antares_jsonld::core_context();
    let own = json!({ "pm25": "https://smartdatamodels.org/pm25", "AQ": "https://smartdatamodels.org/AQ" });
    let own = own.as_object().cloned().unwrap_or_default();
    ctx.merge_object(&own)
        .unwrap_or_else(|e| unreachable!("{e}"));
    let m = Mapping::compile(
        &json!({ "version": 2, "sourceClass": "https://example.org/Sensor",
                 "targetClass": "https://smartdatamodels.org/AQ",
                 "slots": [{ "target": "https://smartdatamodels.org/pm25",
                             "source": "https://example.org/pm2p5", "kind": "rename", "filterable": true }] }),
        &ctx,
        Some(&own),
    )
    .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(m.source_class(), "https://example.org/Sensor");
    assert_eq!(
        m.inner_context(),
        &json!({ "jcSrc0": "https://example.org/pm2p5", "jcSrcClass": "https://example.org/Sensor" })
    );
    assert!(
        m.invert(&pairs(&[("q", "pm25>1"), ("type", "AQ")])).is_ok(),
        "the view's terms"
    );
    let mut e = json!({ "@context": {}, "id": "urn:x", "type": "jcSrcClass", "jcSrc0": 3 });
    m.rebuild(&mut e, &Keep::default());
    assert_eq!(
        e,
        json!({ "@context": own, "id": "urn:x", "type": "AQ", "pm25": 3 })
    );
}

#[test]
fn a_refusal_in_the_inner_terms_is_reworded_into_the_target_model() {
    let m = mapping();
    let text = format!(
        "{} has no attribute {} or {}",
        "jcSrcClass",
        term(&m, "teplota"),
        term(&m, "nazov")
    );
    assert_eq!(
        m.reword(&text),
        "AirQualityObserved has no attribute temperature or an attribute",
        "an input only an expression reads has no target name of its own"
    );
}

#[test]
fn what_two_slots_or_two_literals_share_is_asked_once() {
    let m = mapping();
    let inv = m
        .invert(&pairs(&[("attrs", "label,reliability,pm25,pm25")]))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let asked = format!(
        "{},{},{}",
        term(&m, "nazov"),
        term(&m, "spolahlivost"),
        term(&m, "pm2p5")
    );
    assert_eq!(inv.pairs, [("attrs".to_owned(), asked)]);
    let twice = computed(&json!([slot_of(
        "double",
        &json!({ "binary": "+", "left": { "slot": "a" }, "right": { "slot": "a" } })
    )]));
    let inv = twice
        .invert(&pairs(&[("attrs", "double")]))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(inv.pairs, [("attrs".to_owned(), term(&twice, "a"))]);
    assert_eq!(
        sent(&m, "reliability==\"high\",\"high\""),
        Ok("spolahlivost==\"vysoka\",\"high\"".to_owned())
    );
}

#[test]
fn a_number_or_a_boolean_literal_inverts_through_a_table_by_its_text() {
    let m = slots(
        &json!([{ "target": "level", "source": "l", "kind": "valueMappings",
        "forward": { "1": 7, "on": true }, "filterable": true }]),
    );
    assert_eq!(sent(&m, "level==7"), Ok("l==1,7".to_owned()));
    assert_eq!(sent(&m, "level==true"), Ok("l==\"on\",true".to_owned()));
}

#[test]
fn a_junction_decides_on_every_operand() {
    let k = |v: Value| json!({ "const": v });
    evaluates(&[
        (
            "and",
            json!({ "boolean": "and", "operands": [k(json!(false)), k(json!(true))] }),
            json!(false),
        ),
        (
            "or",
            json!({ "boolean": "or", "operands": [k(json!(true)), k(json!(false))] }),
            json!(true),
        ),
    ]);
}

#[test]
fn an_inverse_too_large_to_round_is_kept_as_it_is() {
    let m = slots(
        &json!([{ "target": "t", "source": "s", "kind": "unitConversion",
        "factor": 1e-300, "filterable": true }]),
    );
    let q = sent(&m, "t==1").unwrap_or_default();
    let n: f64 = q.trim_start_matches("s==").parse().unwrap_or_default();
    assert!((n - 1.0 / 1e-300).abs() <= 1e284, "{q}: not rounded away");
}

const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

fn iri(name: &str) -> String {
    format!("{DEFAULT}{name}")
}

#[test]
fn the_temporal_parameters_pass_and_an_aggregate_is_refused() {
    let m = mapping();
    let asked = pairs(&[
        ("timerel", "between"),
        ("timeAt", "2026-01-01T00:00:00Z"),
        ("endTimeAt", "2026-02-01T00:00:00Z"),
        ("timeproperty", "observedAt"),
        ("lastN", "3"),
        ("options", "temporalValues"),
        ("attrs", "temperature"),
    ]);
    let inverted = m.invert(&asked).map(|i| i.pairs);
    let mut expected = asked.clone();
    if let Some(attrs) = expected.last_mut() {
        attrs.1 = term(&m, "teplota");
    }
    assert_eq!(inverted, Ok(expected));
    for refused in [
        ("options", "temporalValues,aggregatedValues"),
        ("options", " aggregatedValues"),
        ("aggrMethods", "avg"),
        ("aggrPeriodDuration", "PT1H"),
    ] {
        assert_eq!(
            m.invert(&pairs(&[refused])).map(|i| i.pairs),
            Err(NOT_SERVED),
            "{refused:?}"
        );
    }
}

#[test]
fn every_instance_of_a_history_is_rebuilt_and_what_has_none_is_left_out() {
    let m = mapping();
    let history = json!({
        "id": STATION, "type": "jcSrcClass",
        "teplota": [
            {"type": "Property", "value": 293.15, "observedAt": "2026-01-01T00:00:00Z", "instanceId": "urn:i:1"},
            {"type": "Property", "value": 283.15, "observedAt": "2026-01-02T00:00:00Z", "instanceId": "urn:i:2"}
        ],
        "spolahlivost": {"type": "Property", "values": [["vysoka", "2026-01-01T00:00:00Z"], ["inak", "2026-01-02T00:00:00Z"]]},
        "nazov": [{"type": "Property", "value": "S", "observedAt": "2026-01-01T00:00:00Z"}]
    });
    let mut body = Value::Array(vec![answered(&m, &history)]);
    m.rebuild_history(&mut body, &Keep::default());
    assert_eq!(
        body,
        json!([{
            "id": STATION, "type": "AirQualityObserved",
            "temperature": [
                {"type": "Property", "value": 20.0, "observedAt": "2026-01-01T00:00:00Z", "instanceId": "urn:i:1"},
                {"type": "Property", "value": 10.0, "observedAt": "2026-01-02T00:00:00Z", "instanceId": "urn:i:2"}
            ],
            "reliability": {"type": "Property", "values": [["high", "2026-01-01T00:00:00Z"], ["inak", "2026-01-02T00:00:00Z"]]}
        }]),
        "the constant and the expression over `nazov` have no history"
    );
    let mut one = answered(&m, &history);
    m.rebuild_history(&mut one, &Keep::default());
    assert_eq!(one.get("type"), Some(&json!("AirQualityObserved")));
    let mut current = answered(&m, &stored());
    m.rebuild(&mut current, &Keep::default());
    assert!(current.get("dataProvider").is_some(), "{current}");
}

#[test]
fn a_type_list_names_the_target_class_only_for_the_source_class() {
    let m = mapping();
    for (inner, shown) in [
        (json!(["jcSrcClass"]), json!(["AirQualityObserved"])),
        (json!([iri("MestskySenzor")]), json!(["AirQualityObserved"])),
        (json!(["Other"]), json!([])),
        (json!([]), json!([])),
    ] {
        let mut body = json!({"id": "urn:ngsi-ld:EntityTypeList:1", "type": "EntityTypeList", "typeList": inner});
        m.rebuild_discovery(&mut body, None);
        assert_eq!(
            body,
            json!({"id": "urn:ngsi-ld:EntityTypeList:1", "type": "EntityTypeList", "typeList": shown})
        );
    }
}

#[test]
fn discovery_names_the_slots_whose_sources_the_view_shows() {
    let m = mapping();
    let (teplota, nazov, spolahlivost) = (
        term(&m, "teplota"),
        term(&m, "nazov"),
        term(&m, "spolahlivost"),
    );
    // `nazov` shown alone: `label` also reads `spolahlivost`, so it stays out
    let mut details = json!([
        {"id": iri("MestskySenzor"), "type": "EntityType", "typeName": "jcSrcClass",
         "attributeNames": [teplota, nazov, "kalibracia"]},
        {"id": iri("Other"), "type": "EntityType", "typeName": "Other", "attributeNames": ["x"]}
    ]);
    m.rebuild_discovery(&mut details, None);
    assert_eq!(
        details,
        json!([{"id": iri("AirQualityObserved"), "type": "EntityType", "typeName": "AirQualityObserved",
                "attributeNames": ["temperature", "dataProvider"]}])
    );
    let mut list = json!({"id": "urn:x", "type": "AttributeList",
                          "attributeList": [teplota, nazov, spolahlivost, "kalibracia"]});
    m.rebuild_discovery(&mut list, None);
    assert_eq!(
        list.get("attributeList"),
        Some(&json!([
            "temperature",
            "reliability",
            "dataProvider",
            "label"
        ]))
    );
    let mut none = json!({"id": "urn:x", "type": "AttributeList", "attributeList": ["kalibracia"]});
    m.rebuild_discovery(&mut none, None);
    assert_eq!(
        none.get("attributeList"),
        Some(&json!([])),
        "no source shown: not even the constant"
    );
    let mut empty = json!([]);
    m.rebuild_discovery(&mut empty, None);
    assert_eq!(empty, json!([]));
}

#[test]
fn attribute_entries_are_made_from_their_sources() {
    let m = mapping();
    let mut info = json!({
        "@context": "inner", "id": iri("MestskySenzor"), "type": "EntityTypeInfo",
        "typeName": "jcSrcClass", "entityCount": 2,
        "attributeDetails": [
            {"id": iri("teplota"), "type": "Attribute", "attributeName": term(&m, "teplota"), "attributeTypes": ["Property"]},
            {"id": iri("kalibracia"), "type": "Attribute", "attributeName": "kalibracia", "attributeTypes": ["Property"]}
        ]
    });
    m.rebuild_discovery(&mut info, None);
    assert_eq!(
        info,
        json!({
            "@context": antares_jsonld::CORE_CONTEXT, "id": iri("AirQualityObserved"),
            "type": "EntityTypeInfo", "typeName": "AirQualityObserved", "entityCount": 2,
            "attributeDetails": [
                {"id": iri("temperature"), "type": "Attribute", "attributeName": "temperature", "attributeTypes": ["Property"]},
                {"id": iri("dataProvider"), "type": "Attribute", "attributeName": "dataProvider", "attributeTypes": ["Property"]}
            ]
        })
    );
    let mut details = json!([
        {"id": iri("pocet"), "type": "Attribute", "attributeName": term(&m, "pocet"), "typeNames": ["jcSrcClass"]}
    ]);
    m.rebuild_discovery(&mut details, None);
    assert_eq!(
        details,
        json!([
            {"id": iri("stationCount"), "type": "Attribute", "attributeName": "stationCount", "typeNames": ["AirQualityObserved"]},
            {"id": iri("dataProvider"), "type": "Attribute", "attributeName": "dataProvider", "typeNames": ["AirQualityObserved"]}
        ])
    );
    let (slot, source) = m.source_attribute("pm25").unwrap_or_default();
    assert_eq!(source, term(&m, "pm2p5"));
    let mut one = json!({"id": iri("pm2p5"), "type": "Attribute", "attributeName": source,
                         "attributeCount": 3, "attributeTypes": ["Property"], "typeNames": ["jcSrcClass"]});
    m.rebuild_discovery(&mut one, Some(slot));
    assert_eq!(
        one,
        json!({"id": iri("pm25"), "type": "Attribute", "attributeName": "pm25",
               "attributeCount": 3, "attributeTypes": ["Property"], "typeNames": ["AirQualityObserved"]})
    );
    let mut other = json!({"type": "Unknown", "x": 1});
    m.rebuild_discovery(&mut other, None);
    assert_eq!(other, json!({"type": "Unknown", "x": 1}));
}

#[test]
fn a_discovery_read_names_the_target_class_and_takes_its_own_parameters() {
    let m = mapping();
    assert_eq!(m.source_type("AirQualityObserved"), Some("jcSrcClass"));
    assert_eq!(
        m.source_type(
            "https%3A%2F%2Furi.etsi.org%2Fngsi-ld%2Fdefault-context%2FAirQualityObserved"
        ),
        Some("jcSrcClass")
    );
    for other in ["MestskySenzor", "jcSrcClass", ""] {
        assert_eq!(m.source_type(other), None, "{other}");
    }
    assert_eq!(
        discovery_pairs(&pairs(&[
            ("details", "true"),
            ("local", "true"),
            ("count", "true")
        ])),
        Ok(())
    );
    assert_eq!(discovery_pairs(&pairs(&[("type", "X")])), Err(NOT_SERVED));
    assert_eq!(
        discovery_pairs(&pairs(&[("details", "true"), ("details", "false")])),
        Err(TWICE)
    );
}

#[test]
fn what_discovery_or_a_history_cannot_name_passes_as_it_is() {
    let m = mapping();
    for mut body in [
        json!(null),
        json!("text"),
        json!({"id": iri("x"), "type": "Attribute", "attributeName": "x"}),
    ] {
        let before = body.clone();
        m.rebuild_discovery(&mut body, None);
        assert_eq!(body, before);
    }
    let mut history = answered(
        &m,
        &json!({"id": STATION, "type": "jcSrcClass",
                "teplota": {"type": "Property", "values": [[], [283.15]]}}),
    );
    m.rebuild_history(&mut history, &Keep::default());
    assert_eq!(
        history.get("temperature"),
        Some(&json!({"type": "Property", "values": [[], [10.0]]}))
    );
}

#[test]
fn a_geoquery_without_geoproperty_filters_the_target_location() {
    let m = slots(&json!([
        { "target": "location", "source": "position", "kind": "rename", "filterable": true },
        { "target": "where", "source": "location", "kind": "rename", "filterable": true }
    ]));
    let geo = [
        ("georel", "near;maxDistance==500"),
        ("geometry", "Point"),
        ("coordinates", "[17.1,48.1]"),
    ];
    let geoproperties = |list: &[(&str, &str)]| {
        m.invert(&pairs(list)).map(|i| {
            i.pairs
                .into_iter()
                .filter(|(k, _)| k == "geoproperty" || k == "geometryProperty")
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        geoproperties(&geo),
        Ok(vec![("geoproperty".to_owned(), term(&m, "position"))]),
        "the default location is the target's, read from its source"
    );
    for name in ["geoproperty", "geometryProperty"] {
        let mut named = geo.to_vec();
        named.push((name, "where"));
        assert_eq!(
            geoproperties(&named),
            Ok(vec![(name.to_owned(), term(&m, "location"))]),
            "{name}"
        );
    }
    for refusing in [
        slots(
            &json!([{ "target": "where", "source": "location", "kind": "rename", "filterable": true }]),
        ),
        slots(&json!([{ "target": "location", "source": "position", "kind": "rename" }])),
        slots(
            &json!([{ "target": "location", "source": "p", "kind": "unitConversion", "factor": 2, "filterable": true }]),
        ),
    ] {
        assert_eq!(refusing.invert(&pairs(&geo)).err(), Some(NOT_FILTERABLE));
    }
    assert!(
        m.invert(&pairs(&[("geometry", "Point")])).is_ok(),
        "no georel, no geo-query"
    );
}

#[test]
fn a_slot_the_rules_hide_is_read_by_no_filter() {
    let mut m = slots(&json!([
        { "target": "location", "source": "position", "kind": "rename", "filterable": true },
        { "target": "t", "source": "s", "kind": "rename", "filterable": true }
    ]));
    let geo = [
        ("georel", "near;maxDistance==500"),
        ("geometry", "Point"),
        ("coordinates", "[17.1,48.1]"),
    ];
    assert!(
        m.invert(&pairs(&geo)).is_ok(),
        "shown, the location filters"
    );
    let shown = BTreeSet::from([antares_jsonld::core_context().expand_key("s")]);
    assert_eq!(m.hide(&shown), vec!["location".to_owned()]);
    assert_eq!(
        m.invert(&pairs(&geo)).err(),
        Some(NOT_FILTERABLE),
        "a default geo-query would filter the hidden source"
    );
    for asked in [[("geoproperty", "location")], [("orderBy", "location")]] {
        assert_eq!(
            m.invert(&pairs(&asked)).err(),
            Some(NOT_FILTERABLE),
            "{asked:?}"
        );
    }
    assert!(m.invert(&pairs(&[("q", "t==1"), ("orderBy", "t")])).is_ok());
    // a term on the hidden slot holds for nobody, as in the native view:
    // its source is read for presence only, never compared
    for (q, inner) in [
        ("location==\"x\"", "jcSrc0;!jcSrc0"),
        ("location", "jcSrc0;!jcSrc0"),
        ("!location", "jcSrc0|!jcSrc0"),
        ("t==1;location~=\"a\"", "jcSrc1==1;(jcSrc0;!jcSrc0)"),
    ] {
        assert_eq!(
            m.invert(&pairs(&[("q", q)])).map(|i| i.pairs),
            Ok(vec![("q".to_owned(), inner.to_owned())]),
            "{q}"
        );
    }
}

#[test]
fn each_attribute_kind_is_translated_through_a_mapping() {
    let m = slots(&json!([
        { "target": "v", "source": "a", "kind": "valueMappings", "forward": { "x": "y", "urn:p": "urn:q" } },
        { "target": "l", "source": "b", "kind": "unitConversion", "factor": 10 },
        { "target": "r", "source": "c", "kind": "valueMappings", "forward": { "urn:p": "urn:q" } },
        { "target": "m", "source": "d", "kind": "valueMappings", "forward": { "x": "y" } },
        { "target": "j", "source": "e", "kind": "unitConversion", "factor": 2 },
        { "target": "n", "source": "f", "kind": "rename" }
    ]));
    let normalized = rebuilt(
        &m,
        &json!({ "id": "urn:x", "type": "jcSrcClass",
        "a": { "type": "VocabProperty", "vocab": "x" },
        "b": { "type": "ListProperty", "valueList": [1, 2] },
        "c": { "type": "ListRelationship", "objectList": [{ "object": "urn:p" }] },
        "d": { "type": "LanguageProperty", "languageMap": { "sk": "x" } },
        "e": { "type": "JsonProperty", "json": { "n": 1 } },
        "f": { "type": "LanguageProperty", "languageMap": { "sk": "x" } } }),
    );
    assert_eq!(
        normalized["v"],
        json!({ "type": "VocabProperty", "vocab": "y" })
    );
    assert_eq!(
        normalized["l"],
        json!({ "type": "ListProperty", "valueList": [10.0, 20.0] })
    );
    assert_eq!(
        normalized["r"],
        json!({ "type": "ListRelationship", "objectList": [{ "object": "urn:q" }] })
    );
    assert_eq!(
        normalized.get("m"),
        None,
        "a language map has no value table"
    );
    assert_eq!(normalized.get("j"), None, "a JSON value has no unit");
    assert_eq!(
        normalized["n"],
        json!({ "type": "LanguageProperty", "languageMap": { "sk": "x" } }),
        "a rename carries every kind"
    );
    let concise = rebuilt(
        &m,
        &json!({ "id": "urn:x", "type": "jcSrcClass",
        "a": { "vocab": "x" }, "b": { "valueList": [3] }, "d": { "languageMap": { "sk": "x" } } }),
    );
    assert_eq!(concise["v"], json!({ "vocab": "y" }));
    assert_eq!(concise["l"], json!({ "valueList": [30.0] }));
    assert_eq!(concise.get("m"), None);
    let simplified = rebuilt(
        &m,
        &json!({ "id": "urn:x", "type": "jcSrcClass",
        "a": { "vocab": "x" }, "b": [4, 5], "e": { "json": { "n": 1 } } }),
    );
    assert_eq!(simplified["v"], json!({ "vocab": "y" }));
    assert_eq!(simplified["l"], json!([40.0, 50.0]));
    assert_eq!(simplified.get("j"), None);
    let mut attribute = json!({ "type": "LanguageProperty", "languageMap": { "sk": "x" } });
    assert_eq!(
        m.source_attribute("m")
            .map(|(i, _)| m.rebuild_attribute(i, &mut attribute)),
        Ok(false),
        "Retrieve Attribute Value answers as for an absent Attribute"
    );
}

#[test]
fn history_leaves_out_the_instances_a_mapping_cannot_translate() {
    let m = slots(&json!([
        { "target": "t", "source": "s", "kind": "unitConversion", "factor": 2 },
        { "target": "v", "source": "a", "kind": "valueMappings", "forward": { "x": "y" } }
    ]));
    let mut history = answered(
        &m,
        &json!({ "id": "urn:x", "type": "jcSrcClass",
        "s": { "type": "Property", "values": [[1, "2026-10-22T08:00:00Z"], ["n/a", "2026-10-22T09:00:00Z"]] },
        "a": { "type": "VocabProperty", "vocabs": [["x", "2026-10-22T08:00:00Z"]] } }),
    );
    m.rebuild_history(&mut history, &Keep::default());
    assert_eq!(
        history["t"]["values"],
        json!([[2.0, "2026-10-22T08:00:00Z"]])
    );
    assert_eq!(
        history["v"]["vocabs"],
        json!([["y", "2026-10-22T08:00:00Z"]])
    );
    let mut instances = answered(
        &m,
        &json!({ "id": "urn:x", "type": "jcSrcClass",
        "s": [{ "type": "Property", "value": "n/a" }, { "type": "Property", "value": 3 }] }),
    );
    m.rebuild_history(&mut instances, &Keep::default());
    assert_eq!(
        instances["t"],
        json!([{ "type": "Property", "value": 6.0 }])
    );
    let mut none_left = answered(
        &m,
        &json!({ "id": "urn:x", "type": "jcSrcClass",
        "s": [{ "type": "Property", "value": "n/a" }],
        "a": { "type": "VocabProperty", "vocabs": ["not a pair", ["x", "2026-10-22T08:00:00Z"]] } }),
    );
    m.rebuild_history(&mut none_left, &Keep::default());
    assert_eq!(none_left.get("t"), None, "no instance left: no attribute");
    assert_eq!(
        none_left["v"]["vocabs"],
        json!(["not a pair", ["y", "2026-10-22T08:00:00Z"]]),
        "what is no pair passes as it is"
    );
}

#[test]
fn a_table_keyed_by_a_number_translates_the_same_number_stored_as_a_float() {
    let m = slots(&json!([
        { "target": "v", "source": "a", "kind": "valueMappings", "forward": { "1": "one", "true": "yes" }, "filterable": true }
    ]));
    for (stored, served) in [
        (json!(1.0), json!("one")),
        (json!(1), json!("one")),
        (json!(true), json!("yes")),
        (json!(2.0), json!(2.0)),
    ] {
        let e = rebuilt(
            &m,
            &json!({ "id": "urn:x", "type": "jcSrcClass", "a": stored }),
        );
        assert_eq!(e["v"], served, "{stored}");
    }
    assert_eq!(sent(&m, "v==\"one\""), Ok("a==1,\"one\"".to_owned()));
}

#[test]
fn a_slot_whose_source_no_rule_shows_is_listed_as_hidden() {
    let m = slots(&json!([
        { "target": "t", "source": "s", "kind": "rename" },
        { "target": "u", "source": "gone", "kind": "rename" },
        { "target": "k", "kind": "constant", "value": 1 }
    ]));
    let shown = BTreeSet::from([antares_jsonld::core_context().expand_key("s")]);
    assert_eq!(m.hidden_slots(&shown), vec!["u".to_owned()]);
    assert_eq!(
        m.hidden_slots(&BTreeSet::new()),
        vec!["t".to_owned(), "u".to_owned()]
    );
}

/// F-ANT4-010: an ordering keeps the exact inverse of a unit conversion, so
/// the source just below the boundary is no match; equality still rounds.
#[test]
fn an_inverted_ordering_keeps_the_exact_boundary() {
    let d = Derivation::UnitConversion {
        factor: 3.0,
        offset: 0.0,
    };
    let at =
        |op, n| match invert_comparison(&d, QPath::dotted(vec!["t".into()]), op, QValue::Num(n)) {
            Ok(QNode::Cmp {
                value: QValue::Num(b),
                ..
            }) => b,
            other => panic!("not one comparison: {other:?}"),
        };
    let below: f64 = 0.333_333_333_1;
    for op in [CmpOp::Gt, CmpOp::Ge] {
        let b = at(op, 1.0);
        assert!(
            below < b,
            "{below} matches {op:?} {b} but converts to {}",
            below * 3.0
        );
        assert!((b * 3.0 - 1.0).abs() < 1e-15);
    }
    assert_eq!(at(CmpOp::Eq, 1.0), 0.333_333_333);
}
