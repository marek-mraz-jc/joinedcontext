// SPDX-License-Identifier: EUPL-1.2
//! The query guide on the read tools (T-4486): a view of Events says on `query_entities` that the
//! time axis is the `startDate..endDate` range and how to ask for this week, and names no
//! attribute the view hides; `describe_type` answers one type's guide as text, then JSON (T-4537).
//! A read of Articles leaves out their `content` unless the caller names it (T-4582).

use crate::common::{call, create, fixture, fixture_with, prop, put_view, rpc, READS};
use crate::engines::NoTypeInfo;
use serde_json::{json, Value};
use std::sync::Arc;

fn description<'a>(tools: &'a Value, name: &str) -> &'a str {
    tools
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|t| t.get("name") == Some(&json!(name)))
        .and_then(|t| t.get("description").and_then(Value::as_str))
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_read_tools_say_how_to_ask_each_type() -> Result<(), String> {
    let f = fixture().await?;
    put_view(
        &f.app,
        &json!({"id": "events", "tenant": "city", "selector": ["Event"],
                "rules": [{"types": ["Event"], "attributes": ["startDate", "endDate", "location", "category"]}],
                "operations": READS}),
    )
    .await?;
    let tools = rpc(&f.app, "events", "tools/list", json!({})).await;
    let query = description(tools.result(), "query_entities");
    assert!(
        query.contains("Event: time startDate..endDate (a range)"),
        "{query}"
    );
    assert!(query.contains("q=startDate<"), "{query}");
    assert!(query.contains("Filter on category."), "{query}");
    assert!(!query.contains("organizer"), "a hidden attribute: {query}");
    assert!(!description(tools.result(), "get_entity").contains("Query guide"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn describe_type_answers_the_guide_text_first_and_nothing_for_a_hidden_type(
) -> Result<(), String> {
    let f = fixture().await?;
    put_view(
        &f.app,
        &json!({"id": "events", "tenant": "city", "selector": ["Event"],
                "rules": [{"types": ["Event"], "attributes": ["startDate", "endDate", "location", "category"]}],
                "operations": READS}),
    )
    .await?;
    let tools = rpc(&f.app, "events", "tools/list", json!({})).await;
    assert!(description(tools.result(), "describe_type").contains("time axis"));
    let a = call(&f.app, "events", "describe_type", json!({"type": "Event"})).await;
    assert!(!a.is_error(), "{}", a.text);
    let first = a
        .result()
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        first.contains("\n```text\nEvent: time startDate..endDate (a range)"),
        "{}",
        a.text
    );
    assert_eq!(
        a.structured().pointer("/guide/time"),
        Some(&json!({"start": "startDate", "end": "endDate"}))
    );
    assert!(
        !a.text.contains("organizer"),
        "a hidden attribute: {}",
        a.text
    );
    let hidden = call(
        &f.app,
        "events",
        "describe_type",
        json!({"type": "Vehicle"}),
    )
    .await;
    assert!(hidden.is_error(), "{}", hidden.text);
    assert!(
        hidden.structured().get("guide").is_none(),
        "{}",
        hidden.text
    );
    assert!(!hidden.text.contains("location"), "{}", hidden.text);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn describe_type_a_gate_denies_answers_the_refusal_and_no_guide() -> Result<(), String> {
    let f = fixture_with(Some(Arc::new(NoTypeInfo))).await?;
    let a = call(&f.app, "cars", "describe_type", json!({"type": "Car"})).await;
    assert!(a.is_error(), "{}", a.text);
    assert!(a.text.contains("403"), "{}", a.text);
    assert!(a.structured().get("guide").is_none(), "{}", a.text);
    Ok(())
}

/// Whether any Entity of `a`'s answer carries `content`.
fn bodies(a: &crate::common::Answer) -> bool {
    let s = a.structured();
    let one = s.get("entity").into_iter();
    let many = s
        .get("entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    one.chain(many).any(|e| e.get("content").is_some())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_article_body_comes_only_when_asked_and_never_past_the_view() -> Result<(), String> {
    let f = fixture().await?;
    for n in 0..3 {
        create(
            &f.app,
            "news",
            &json!({"id": format!("urn:ngsi-ld:Article:{n}"), "type": "Article",
                    "name": prop(&json!(format!("Headline {n}"))),
                    "content": prop(&json!("body ".repeat(500)))}),
        )
        .await?;
    }
    for (id, attributes) in [
        ("articles", json!(["name", "content"])),
        ("headlines", json!(["name"])),
    ] {
        put_view(
            &f.app,
            &json!({"id": id, "tenant": "news", "selector": ["Article"],
                    "rules": [{"types": ["Article"], "attributes": attributes}],
                    "operations": READS}),
        )
        .await?;
    }
    let tools = rpc(&f.app, "articles", "tools/list", json!({})).await;
    assert!(
        description(tools.result(), "query_entities")
            .contains("content is left out unless asked for in attrs or pick."),
        "{}",
        tools.text
    );
    let plain = call(
        &f.app,
        "articles",
        "query_entities",
        json!({"type": ["Article"]}),
    )
    .await;
    assert!(!plain.is_error(), "{}", plain.text);
    assert!(plain.text.contains("Headline 0"), "{}", plain.text);
    assert!(!bodies(&plain), "{}", plain.text);
    let one = call(
        &f.app,
        "articles",
        "get_entity",
        json!({"id": "urn:ngsi-ld:Article:1"}),
    )
    .await;
    assert!(
        one.text.contains("Headline 1") && !bodies(&one),
        "{}",
        one.text
    );
    for asked in [
        json!({"attrs": ["content"]}),
        json!({"pick": ["name", "content"]}),
    ] {
        let mut args = asked.clone();
        if let Some(m) = args.as_object_mut() {
            m.insert("type".into(), json!(["Article"]));
        }
        let a = call(&f.app, "articles", "query_entities", args).await;
        assert!(bodies(&a), "{asked}: {}", a.text);
    }
    let hidden = call(
        &f.app,
        "headlines",
        "query_entities",
        json!({"type": ["Article"], "attrs": ["name", "content"]}),
    )
    .await;
    assert!(!bodies(&hidden), "{}", hidden.text);
    Ok(())
}
