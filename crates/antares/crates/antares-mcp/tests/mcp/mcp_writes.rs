// SPDX-License-Identifier: EUPL-1.2
//! Write tools run only after the person confirmed the call (MCP 3.4,
//! D-11; MC4, MC12): the first call asks, the retry with the sealed
//! `requestState` runs once, and the gate still decides.

use crate::common::{
    answered, call, confirmable, fixture, fixture_with, prop, tool_names, Answer, HOOKS,
};
use crate::engines::{KeepW1, NoDeletes};
use axum::Router;
use serde_json::{json, Value};
use std::sync::Arc;

/// The `requestState` of an `InputRequiredResult`, and its question.
pub(crate) fn asked(a: &Answer) -> Result<(String, String), String> {
    let r = a.result();
    if r.get("resultType") != Some(&json!("input_required")) {
        return Err(format!("not asked: {}", a.text));
    }
    let message = r
        .pointer("/inputRequests/confirm/params/message")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("no question: {}", a.text))?;
    let state = r
        .get("requestState")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("no requestState: {}", a.text))?;
    Ok((state.to_owned(), message.to_owned()))
}

/// `tool` with `args` on `view`, asked and confirmed.
async fn confirmed(app: &Router, view: &str, tool: &str, args: &Value) -> Result<Answer, String> {
    let first = confirmable(app, view, tool, args.clone(), json!({})).await;
    let (state, _) = asked(&first)?;
    Ok(confirmable(app, view, tool, args.clone(), answered(&state, true)).await)
}

/// The tool error's text.
fn error_text(a: &Answer) -> String {
    a.result()
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Whether `id` can be read through view `writes`.
async fn exists(app: &Router, id: &str) -> bool {
    !call(app, "writes", "get_entity", json!({"id": id}))
        .await
        .is_error()
}

fn car(n: &str, speed: i64) -> Value {
    json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": "Car", "speed": prop(&json!(speed))})
}

#[tokio::test(flavor = "multi_thread")]
async fn every_write_tool_runs_once_the_person_confirmed() -> Result<(), String> {
    let f = fixture().await?;
    let w2 = "urn:ngsi-ld:Car:w2";
    let created = confirmed(
        &f.app,
        "writes",
        "create_entity",
        &json!({"entity": car("w2", 5)}),
    )
    .await?;
    assert_eq!(
        created.structured(),
        &json!({"created": w2}),
        "{}",
        created.text
    );
    let steps = [
        (
            "update_attributes",
            json!({"id": w2, "attributes": {"speed": prop(&json!(6))}}),
        ),
        (
            "append_attributes",
            json!({"id": w2, "attributes": {"colour": prop(&json!("blue"))}, "noOverwrite": true}),
        ),
        (
            "update_attribute",
            json!({"id": w2, "attrId": "speed", "attribute": {"value": 7}}),
        ),
        (
            "replace_attribute",
            json!({"id": w2, "attrId": "speed", "attribute": prop(&json!(8))}),
        ),
        (
            "merge_entity",
            json!({"id": w2, "fragment": {"colour": prop(&json!("green"))}}),
        ),
        ("delete_attribute", json!({"id": w2, "attrId": "colour"})),
        ("replace_entity", json!({"id": w2, "entity": car("w2", 9)})),
        (
            "batch_create",
            json!({"entities": [car("w3", 1), car("w4", 1)]}),
        ),
        (
            "batch_update",
            json!({"entities": [car("w4", 3)], "noOverwrite": false}),
        ),
        ("batch_merge", json!({"entities": [car("w4", 4)]})),
        (
            "batch_delete",
            json!({"ids": ["urn:ngsi-ld:Car:w3", "urn:ngsi-ld:Car:w4"]}),
        ),
    ];
    for (tool, args) in steps {
        let a = confirmed(&f.app, "writes", tool, &args).await?;
        assert!(!a.is_error(), "{tool}: {}", a.text);
    }
    // a view decides an upsert member by member (ADR-0023 rule 7, T-3594):
    // the absent w3 is created in the view
    let upsert = json!({"entities": [car("w3", 2)], "options": "update"});
    let a = confirmed(&f.app, "writes", "batch_upsert", &upsert).await?;
    assert!(!a.is_error(), "{}", a.text);
    let a = confirmed(&f.app, "writes", "get_entity", &json!({"id": w2})).await;
    assert!(a.is_err(), "a read is never asked");
    let speed = call(&f.app, "writes", "get_entity", json!({"id": w2})).await;
    assert_eq!(
        speed.structured().pointer("/entity/speed/value"),
        Some(&json!(9))
    );
    assert!(exists(&f.app, "urn:ngsi-ld:Car:w3").await);
    let deleted = confirmed(&f.app, "writes", "delete_entity", &json!({"id": w2})).await?;
    assert_eq!(deleted.structured(), &json!({}), "{}", deleted.text);
    assert!(!exists(&f.app, w2).await);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn every_subscription_tool_runs_once_the_person_confirmed() -> Result<(), String> {
    let f = fixture().await?;
    let sub = json!({"type": "Subscription", "entities": [{"type": "Car"}],
                     "watchedAttributes": ["speed"],
                     "notification": {"attributes": ["speed"], "endpoint": {"uri": format!("{HOOKS}a")}}});
    let made = confirmed(
        &f.app,
        "writes",
        "create_subscription",
        &json!({"subscription": sub}),
    )
    .await?;
    let id = made
        .structured()
        .get("created")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("no subscription id: {}", made.text))?
        .to_owned();
    let changed = confirmed(
        &f.app,
        "writes",
        "update_subscription",
        &json!({"id": id, "fragment": {"description": "slower"}}),
    )
    .await?;
    assert!(!changed.is_error(), "{}", changed.text);
    let gone = confirmed(&f.app, "writes", "delete_subscription", &json!({"id": id})).await?;
    assert!(!gone.is_error(), "{}", gone.text);
    Ok(())
}

/// MC4: the first call only asks; a refusal or a "no" writes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_without_confirmation_writes_nothing() -> Result<(), String> {
    let f = fixture().await?;
    let args = json!({"entity": {"id": "urn:ngsi-ld:Car:w9", "type": "Car",
                                 "speed": prop(&json!("drop every table"))}});
    let first = confirmable(&f.app, "writes", "create_entity", args.clone(), json!({})).await;
    let (state, question) = asked(&first)?;
    assert_eq!(
        question,
        "Allow create_entity (create_entity); on: urn:ngsi-ld:Car:w9; Attributes: speed?"
    );
    assert!(!first.text.contains("drop every table"), "{}", first.text);
    assert!(!exists(&f.app, "urn:ngsi-ld:Car:w9").await);
    let no = confirmable(
        &f.app,
        "writes",
        "create_entity",
        args.clone(),
        answered(&state, false),
    )
    .await;
    assert!(no.is_error());
    assert_eq!(
        error_text(&no),
        "the person did not confirm; nothing was written"
    );
    let (state, _) =
        asked(&confirmable(&f.app, "writes", "create_entity", args.clone(), json!({})).await)?;
    let declined =
        json!({"requestState": state, "inputResponses": {"confirm": {"action": "decline"}}});
    assert!(
        confirmable(&f.app, "writes", "create_entity", args.clone(), declined)
            .await
            .is_error()
    );
    let (state, _) =
        asked(&confirmable(&f.app, "writes", "create_entity", args.clone(), json!({})).await)?;
    let silent = json!({"requestState": state});
    assert!(confirmable(&f.app, "writes", "create_entity", args, silent)
        .await
        .is_error());
    assert!(!exists(&f.app, "urn:ngsi-ld:Car:w9").await);
    Ok(())
}

/// A "no" spends nothing (F-ANT1-003): the person may still say yes to the
/// same question, so declines cannot fill the spent set.
#[tokio::test(flavor = "multi_thread")]
async fn a_decline_leaves_the_request_state_unspent() -> Result<(), String> {
    let f = fixture().await?;
    let args = json!({"entity": car("w8", 1)});
    let (state, _) =
        asked(&confirmable(&f.app, "writes", "create_entity", args.clone(), json!({})).await)?;
    let no = confirmable(
        &f.app,
        "writes",
        "create_entity",
        args.clone(),
        answered(&state, false),
    )
    .await;
    assert!(no.is_error());
    let yes = confirmable(
        &f.app,
        "writes",
        "create_entity",
        args,
        answered(&state, true),
    )
    .await;
    assert!(!yes.is_error(), "{}", yes.text);
    assert!(exists(&f.app, "urn:ngsi-ld:Car:w8").await);
    Ok(())
}

/// MC4, MCP section 10 rule 10: one confirmation answers one call once.
#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_request_state_is_refused() -> Result<(), String> {
    let f = fixture().await?;
    let args = json!({"id": "urn:ngsi-ld:Car:w1"});
    let (state, _) =
        asked(&confirmable(&f.app, "writes", "delete_entity", args.clone(), json!({})).await)?;
    let once = confirmable(
        &f.app,
        "writes",
        "delete_entity",
        args.clone(),
        answered(&state, true),
    )
    .await;
    assert!(!once.is_error(), "{}", once.text);
    assert!(confirmed(
        &f.app,
        "writes",
        "create_entity",
        &json!({"entity": car("w1", 1)})
    )
    .await
    .is_ok());
    let again = confirmable(
        &f.app,
        "writes",
        "delete_entity",
        args,
        answered(&state, true),
    )
    .await;
    assert!(again.is_error());
    assert_eq!(
        error_text(&again),
        "the requestState does not answer this call; call the tool again without it"
    );
    assert!(exists(&f.app, "urn:ngsi-ld:Car:w1").await);
    Ok(())
}

/// A forged state, or a real one for other arguments or another caller,
/// answers nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_tampered_request_state_is_refused() -> Result<(), String> {
    let f = fixture().await?;
    let args = json!({"id": "urn:ngsi-ld:Car:w1"});
    let (state, _) =
        asked(&confirmable(&f.app, "writes", "delete_entity", args.clone(), json!({})).await)?;
    let mut forged = state.clone().into_bytes();
    if let Some(last) = forged.last_mut() {
        *last = if *last == b'A' { b'B' } else { b'A' };
    }
    let forged = String::from_utf8(forged).map_err(|e| e.to_string())?;
    let other = json!({"id": "urn:ngsi-ld:Car:w2"});
    for (args, state) in [
        (args.clone(), forged.as_str()),
        (args.clone(), "rs1.e30.AAAA"),
        (other, state.as_str()),
    ] {
        let a = confirmable(
            &f.app,
            "writes",
            "delete_entity",
            args,
            answered(state, true),
        )
        .await;
        assert!(a.is_error(), "{}", a.text);
    }
    let elsewhere = confirmable(
        &f.app,
        "writes",
        "create_entity",
        json!({"entity": car("w1", 1)}),
        answered(&state, true),
    )
    .await;
    assert!(elsewhere.is_error(), "a state answers its own tool only");
    assert!(exists(&f.app, "urn:ngsi-ld:Car:w1").await);
    let real = confirmable(
        &f.app,
        "writes",
        "delete_entity",
        args,
        answered(&state, true),
    )
    .await;
    assert!(
        !real.is_error(),
        "the failed tries spent nothing: {}",
        real.text
    );
    Ok(())
}

/// MC4: a confirmation is bound to the view that asked.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_state_for_one_view_is_refused_on_another() -> Result<(), String> {
    let f = fixture().await?;
    let args = json!({"id": "urn:ngsi-ld:Car:w1"});
    let (state, _) =
        asked(&confirmable(&f.app, "writes", "delete_entity", args.clone(), json!({})).await)?;
    let a = confirmable(
        &f.app,
        "writes2",
        "delete_entity",
        args,
        answered(&state, true),
    )
    .await;
    assert!(a.is_error(), "{}", a.text);
    assert!(exists(&f.app, "urn:ngsi-ld:Car:w1").await);
    Ok(())
}

/// D-3: a write tool is listed where the view allows its operation only,
/// and is no read-only tool.
#[tokio::test(flavor = "multi_thread")]
async fn write_tools_are_offered_only_where_the_view_allows_them() -> Result<(), String> {
    let f = fixture().await?;
    assert!(!tool_names(&f.app, "cars")
        .await
        .iter()
        .any(|t| t == "create_entity"));
    let hidden = confirmable(
        &f.app,
        "cars",
        "create_entity",
        json!({"entity": car("x", 1)}),
        json!({}),
    )
    .await;
    assert_eq!(
        hidden.error().get("code"),
        Some(&json!(-32602)),
        "{}",
        hidden.text
    );
    let listed = crate::common::rpc(&f.app, "writes", "tools/list", json!({})).await;
    let tools = listed
        .result()
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(tools.len(), 33, "{}", listed.text);
    let hint = |name: &str, hint: &str| {
        tools
            .iter()
            .find(|t| t.get("name") == Some(&json!(name)))
            .and_then(|t| t.pointer(&format!("/annotations/{hint}")))
            .cloned()
    };
    assert_eq!(hint("create_entity", "readOnlyHint"), Some(json!(false)));
    assert_eq!(hint("get_entity", "readOnlyHint"), Some(json!(true)));
    // MCP: a write that only adds is not destructive; one that removes or
    // overwrites says so, a read says nothing
    for additive in ["create_entity", "create_subscription", "batch_create"] {
        assert_eq!(
            hint(additive, "destructiveHint"),
            Some(json!(false)),
            "{additive}"
        );
    }
    for destructive in [
        "delete_entity",
        "delete_attribute",
        "replace_entity",
        "update_attributes",
        "batch_delete",
    ] {
        assert_eq!(
            hint(destructive, "destructiveHint"),
            Some(json!(true)),
            "{destructive}"
        );
    }
    assert_eq!(hint("get_entity", "destructiveHint"), None);
    Ok(())
}

/// MCP 3.4: a client that cannot ask the person is not asked to.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_cannot_ask_the_person_gets_a_tool_error() -> Result<(), String> {
    let f = fixture().await?;
    let a = call(
        &f.app,
        "writes",
        "delete_entity",
        json!({"id": "urn:ngsi-ld:Car:w1"}),
    )
    .await;
    assert!(a.is_error(), "{}", a.text);
    assert_eq!(
        error_text(&a),
        "this tool needs a client that can ask the person"
    );
    let params = json!({"name": "delete_entity", "arguments": {"id": "urn:ngsi-ld:Car:w1"}});
    for capabilities in [json!({"elicitation": {"url": {}}}), json!({"sampling": {}})] {
        let a = crate::common::call_as(&f.app, "writes", params.clone(), &capabilities).await;
        assert_eq!(
            error_text(&a),
            "this tool needs a client that can ask the person"
        );
    }
    let (state, _) = asked(
        &crate::common::call_as(&f.app, "writes", params, &json!({"elicitation": {}})).await,
    )?;
    assert_ne!(state, "", "an empty elicitation declares a form");
    let bad = confirmable(
        &f.app,
        "writes",
        "delete_entity",
        json!({"id": "Car:w1"}),
        json!({}),
    )
    .await;
    assert_eq!(
        error_text(&bad),
        "argument id: must be an NGSI-LD Entity id"
    );
    assert!(exists(&f.app, "urn:ngsi-ld:Car:w1").await);
    Ok(())
}

/// MC12, MCP section 10 rule 7.
#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_to_an_endpoint_outside_the_allowlist_is_refused_before_the_gate(
) -> Result<(), String> {
    let f = fixture().await?;
    let sub = |uri: &str| {
        json!({"subscription": {"type": "Subscription", "entities": [{"type": "Car"}],
                                "notification": {"endpoint": {"uri": uri}}}})
    };
    for uri in [
        "https://evil.example/a",
        "https://hooks.example.evil/a",
        "https://hooks.example",
    ] {
        let a = confirmable(&f.app, "writes", "create_subscription", sub(uri), json!({})).await;
        assert_eq!(
            error_text(&a),
            "argument subscription.notification.endpoint.uri: not an endpoint this deployment lets a tool notify",
            "{uri}"
        );
    }
    let fragment = json!({"id": "urn:ngsi-ld:Subscription:s",
                          "fragment": {"notification": {"endpoint": {"uri": "https://evil.example/"}}}});
    let a = confirmable(&f.app, "writes", "update_subscription", fragment, json!({})).await;
    assert!(a.is_error(), "{}", a.text);
    let (_, question) = asked(
        &confirmable(
            &f.app,
            "writes",
            "create_subscription",
            sub(&format!("{HOOKS}a")),
            json!({}),
        )
        .await,
    )?;
    assert_eq!(
        question,
        "Allow create_subscription (create_subscription); notifying: https://hooks.example/a?"
    );
    Ok(())
}

/// MC4, MCP section 10 rule 6: a write the engine denies is refused before
/// the person is asked.
/// Decided (worker-4, owner may override): the first call asks the probe
/// (the engine's answer for the operation, D-3), since running the gate of
/// the operation itself would run the write; a denial that depends on the
/// Entity is the gate's answer to the confirmed retry, below.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_the_engine_denies_never_asks_the_person() -> Result<(), String> {
    let f = fixture_with(Some(Arc::new(NoDeletes))).await?;
    let a = confirmable(
        &f.app,
        "writes",
        "delete_entity",
        json!({"id": "urn:ngsi-ld:Car:w1"}),
        json!({}),
    )
    .await;
    assert_eq!(a.error().get("code"), Some(&json!(-32602)), "{}", a.text);
    assert!(asked(&a).is_err());
    assert!(exists(&f.app, "urn:ngsi-ld:Car:w1").await);
    Ok(())
}

/// MCP section 10 rule 6: the confirmation is in addition to the gate.
#[tokio::test(flavor = "multi_thread")]
async fn a_confirmed_write_the_gate_denies_is_refused() -> Result<(), String> {
    let f = fixture_with(Some(Arc::new(KeepW1))).await?;
    let a = confirmed(
        &f.app,
        "writes",
        "delete_entity",
        &json!({"id": "urn:ngsi-ld:Car:w1"}),
    )
    .await?;
    assert!(a.is_error(), "{}", a.text);
    assert_eq!(a.structured().pointer("/error/status"), Some(&json!(403)));
    assert!(exists(&f.app, "urn:ngsi-ld:Car:w1").await);
    Ok(())
}
