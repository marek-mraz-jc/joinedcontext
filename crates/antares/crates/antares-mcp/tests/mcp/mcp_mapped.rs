// SPDX-License-Identifier: EUPL-1.2
//! A view of another model through MCP (T-3817, gap G18): `vehicles` serves
//! the fixture's Cars below 80 as `Vehicle`s. Its tools take and answer the
//! target model, only the reads a mapping runs are offered, and nothing of
//! the source model is named: not `speed`, `colour`, `secret` nor `Car`.

use crate::common::{call, fixture, put_view, rpc, tool_names, Answer, Fixture};
use serde_json::{json, Value};

async fn vehicles() -> Result<Fixture, String> {
    let f = fixture().await?;
    put_view(
        &f.app,
        &json!({
            "id": "vehicles", "tenant": "city", "selector": ["Car"],
            "rules": [{"types": ["Car"], "q": "speed<80", "attributes": ["speed", "colour"]}],
            "operations": antares_ops::mapping::READS.iter().map(|op| op.id()).collect::<Vec<_>>(),
            "mapping": {
                "version": 2, "sourceClass": "Car", "targetClass": "Vehicle",
                "slots": [
                    {"target": "velocity", "source": "speed", "kind": "unitConversion",
                     "factor": 2, "filterable": true},
                    {"target": "paint", "source": "colour", "kind": "valueMappings",
                     "forward": {"red": "rot"}, "filterable": true},
                    {"target": "hidden", "source": "secret", "kind": "rename", "filterable": true},
                    {"target": "maker", "kind": "constant", "value": "acme"}
                ]
            }
        }),
    )
    .await?;
    Ok(f)
}

fn speaks_the_target_model(a: &Answer) {
    for leak in [
        "speed", "colour", "secret", "s3cr3t", "hidden", "\"Car\"", "jcSrc",
    ] {
        assert!(!a.text.contains(leak), "{leak} in {}", a.text);
    }
}

fn ids(a: &Answer) -> Vec<String> {
    a.structured()
        .get("entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// The tools are the mapping's reads, their schemas in the target model.
#[tokio::test(flavor = "multi_thread")]
async fn a_mapped_view_lists_its_reads_in_the_target_model() -> Result<(), String> {
    let f = vehicles().await?;
    let names = tool_names(&f.app, "vehicles").await;
    assert!(names.contains(&"query_entities".to_owned()), "{names:?}");
    assert!(names.contains(&"get_entity".to_owned()), "{names:?}");
    for absent in [
        "batch_query",
        "create_entity",
        "update_attributes",
        "create_subscription",
    ] {
        assert!(
            !names.contains(&absent.to_owned()),
            "{absent} offered: {names:?}"
        );
    }
    let list = rpc(&f.app, "vehicles", "tools/list", json!({})).await;
    speaks_the_target_model(&list);
    let query = list
        .result()
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|t| t.get("name") == Some(&json!("query_entities")))
        .cloned()
        .unwrap_or_default();
    let schema = query.get("inputSchema").cloned().unwrap_or_default();
    assert_eq!(
        schema.pointer("/properties/type/items/enum"),
        Some(&json!(["Vehicle"]))
    );
    assert_eq!(
        schema.pointer("/properties/attrs/items/enum"),
        Some(&json!(["maker", "paint", "velocity"])),
        "`hidden` reads `secret`, which the view does not show"
    );
    Ok(())
}

/// A filter and a read by id in the target model run in the source model
/// and answer in the target model.
#[tokio::test(flavor = "multi_thread")]
async fn a_mapped_view_answers_in_the_target_model() -> Result<(), String> {
    let f = vehicles().await?;
    let a = call(
        &f.app,
        "vehicles",
        "query_entities",
        json!({"type": ["Vehicle"]}),
    )
    .await;
    assert!(!a.is_error(), "{}", a.text);
    assert_eq!(
        ids(&a),
        ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:2"],
        "Car:3 is out of the view"
    );
    speaks_the_target_model(&a);

    let a = call(
        &f.app,
        "vehicles",
        "query_entities",
        json!({"type": ["Vehicle"], "q": "velocity<50"}),
    )
    .await;
    assert_eq!(ids(&a), ["urn:ngsi-ld:Car:1"], "{}", a.text);

    let a = call(
        &f.app,
        "vehicles",
        "get_entity",
        json!({"id": "urn:ngsi-ld:Car:1"}),
    )
    .await;
    assert!(!a.is_error(), "{}", a.text);
    assert!(
        a.text.contains("velocity") && a.text.contains("rot"),
        "{}",
        a.text
    );
    speaks_the_target_model(&a);

    let a = call(
        &f.app,
        "vehicles",
        "get_entity",
        json!({"id": "urn:ngsi-ld:Car:3"}),
    )
    .await;
    assert!(a.is_error(), "{}", a.text);
    speaks_the_target_model(&a);
    Ok(())
}

/// A filter on a source name or an unshown slot finds nothing to name.
#[tokio::test(flavor = "multi_thread")]
async fn a_source_name_is_no_argument_of_a_mapped_view() -> Result<(), String> {
    let f = vehicles().await?;
    for args in [
        json!({"type": ["Car"]}),
        json!({"type": ["Vehicle"], "attrs": ["speed"]}),
        json!({"type": ["Vehicle"], "q": "speed<50"}),
    ] {
        let a = call(&f.app, "vehicles", "query_entities", args.clone()).await;
        assert!(a.is_error() || ids(&a).is_empty(), "{args}: {}", a.text);
        assert!(!a.text.contains("s3cr3t"), "{args}: {}", a.text);
    }
    Ok(())
}
