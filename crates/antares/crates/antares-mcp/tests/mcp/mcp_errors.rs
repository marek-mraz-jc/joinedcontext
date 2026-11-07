// SPDX-License-Identifier: EUPL-1.2
//! What a refusal says (MC18).

use crate::common::{call, fixture};
use serde_json::json;

/// MC18: a refused argument names the rule, a failed operation its status
/// and title; neither carries a hidden name, a value or an inner detail.
#[tokio::test(flavor = "multi_thread")]
async fn errors_carry_no_hidden_name_and_no_inner_detail() -> Result<(), String> {
    let f = fixture().await?;
    let hidden_attr = call(
        &f.app,
        "cars",
        "get_entity",
        json!({"id": "urn:ngsi-ld:Car:1", "attrs": ["secret"]}),
    )
    .await;
    assert!(hidden_attr.is_error(), "{}", hidden_attr.text);
    assert!(!hidden_attr.text.contains("secret"), "{}", hidden_attr.text);

    let bad_q = call(
        &f.app,
        "cars",
        "query_entities",
        json!({"type": ["Car"], "q": ")xyzzy("}),
    )
    .await;
    assert!(bad_q.is_error(), "{}", bad_q.text);
    assert!(bad_q.text.contains("400"), "{}", bad_q.text);
    assert!(!bad_q.text.contains("xyzzy"), "{}", bad_q.text);
    assert_eq!(
        bad_q.structured().pointer("/error/status"),
        Some(&json!(400)),
        "{}",
        bad_q.text
    );
    assert!(bad_q.structured().pointer("/error/detail").is_none());

    let out_of_view = call(
        &f.app,
        "cars",
        "get_entity",
        json!({"id": "urn:ngsi-ld:Car:3"}),
    )
    .await;
    let missing = call(
        &f.app,
        "cars",
        "get_entity",
        json!({"id": "urn:ngsi-ld:Car:404"}),
    )
    .await;
    assert!(
        out_of_view.is_error() && out_of_view.text.contains("404"),
        "{}",
        out_of_view.text
    );
    assert_eq!(out_of_view.text, missing.text);
    assert!(!out_of_view.text.contains("Car:3"), "{}", out_of_view.text);
    Ok(())
}

/// F-R2ANT2-010: a tool call reads its inner answer through the broker's
/// answer bound and pool, as every other surface does: past the bound the
/// call fails with 413 instead of buffering on the side.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_call_reads_its_answer_under_the_answer_bound() -> Result<(), String> {
    let broker = antares_ops::test_kit::TestBroker::with_max_answer_bytes(4096).await?;
    let app = broker.serve(antares_mcp::Mcp)?;
    crate::common::create(
        &app,
        "small",
        &json!({"id": "urn:ngsi-ld:Car:long", "type": "Car",
                "note": crate::common::prop(&json!("n".repeat(10_000)))}),
    )
    .await?;
    crate::common::put_view(
        &app,
        &json!({"id": "small", "tenant": "small", "selector": ["Car"],
                "rules": [{"types": ["Car"], "attributes": ["note"]}],
                "operations": ["query_entities"]}),
    )
    .await?;
    let answer = call(&app, "small", "query_entities", json!({"type": ["Car"]})).await;
    assert!(answer.is_error(), "{}", answer.text);
    assert_eq!(
        answer.structured().pointer("/error/status"),
        Some(&json!(413)),
        "{}",
        answer.text
    );
    Ok(())
}
