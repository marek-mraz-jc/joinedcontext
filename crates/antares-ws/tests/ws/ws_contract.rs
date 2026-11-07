// SPDX-License-Identifier: EUPL-1.2
//! ADR-0026 rule 11: `docs/api/asyncapi-ws.yaml` describes every message
//! the crate writes and reads exactly (T-3518). The sessions of the other
//! modules check each message they receive; here every builder, every
//! closing reason and a corpus of client messages are held against it.

use crate::common::asyncapi::{check, check_server, doc, resolve};
use antares_ws::envelope::{self, read};
use serde_json::{json, Value};

const CONNECTION: &str = "abcdefghijklmnopqrstuvwxyz";
const TOKEN: &str = "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst";

fn parsed(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

#[test]
fn every_message_of_rules_3_to_7_is_in_the_document() -> Result<(), String> {
    let messages = doc()?
        .pointer("/components/messages")
        .and_then(Value::as_object)
        .ok_or("no components.messages")?;
    let mut names: Vec<&str> = messages.keys().map(String::as_str).collect();
    names.sort_unstable();
    let want = [
        "closing",
        "gap",
        "notification",
        "result",
        "resume",
        "subscribe",
        "unsubscribe",
        "update",
        "welcome",
    ];
    assert_eq!(names, want);
    for (name, m) in messages {
        assert_eq!(m.get("name"), Some(&json!(format!("x-{name}"))), "{name}");
    }
    Ok(())
}

#[test]
fn every_server_message_the_crate_builds_is_described() -> Result<(), String> {
    let subscription = json!({"id": "urn:ngsi-ld:Subscription:1", "type": "Subscription"});
    let rendered = json!({
        "metadata": {"Content-Type": "application/json", "Link": "<urn:ctx>", "NGSILD-Tenant": "city"},
        "body": {
            "id": "urn:ngsi-ld:Notification:1", "type": "Notification",
            "subscriptionId": "urn:ngsi-ld:Subscription:1",
            "notifiedAt": "2026-10-10T00:00:00Z", "data": [{"id": "urn:ngsi-ld:Car:1", "type": "Car"}]
        }
    });
    let built = [
        envelope::welcome(CONNECTION, TOKEN),
        envelope::result(Some("1"), 201, Some(subscription)),
        envelope::result(None, 204, None),
        envelope::result(Some("2"), 400, Some(envelope::bad_request("no"))),
        envelope::result(Some("3"), 404, Some(envelope::not_found())),
        envelope::notification(&rendered, 1, "urn:ngsi-ld:Subscription:1", 0),
        envelope::gap(2, 9),
        // every reason the socket closes with (socket.rs)
        envelope::closing("lifetime"),
        envelope::closing("overflow"),
        envelope::closing("view-removed"),
        envelope::closing("idle"),
    ];
    for text in built {
        check_server(&parsed(&text)?)?;
    }
    Ok(())
}

/// The validator is not a pass-through: each of these breaks one rule of
/// the document and is refused.
#[test]
fn a_message_the_document_does_not_describe_is_refused() {
    let broken = [
        json!({"metadata": {"x-op": "x-welcome", "x-connection": "UPPER", "x-resume-token": TOKEN}}),
        json!({"metadata": {"x-op": "x-welcome", "x-connection": CONNECTION}}),
        json!({"metadata": {"x-op": "x-result", "x-status": 99}}),
        json!({"metadata": {"x-op": "x-result", "x-status": 200}, "body": {"type": "Subscription", "title": "t", "status": 400}}),
        json!({"metadata": {"x-op": "x-gap", "x-from": 0, "x-to": 1}}),
        json!({"metadata": {"x-op": "x-closing", "x-reason": "bored"}}),
        json!({"metadata": {"x-op": "x-closing", "x-reason": "idle", "x-more": 1}}),
        json!({"metadata": {"x-op": "x-notification", "Content-Type": "application/json", "x-seq": 1,
            "x-subscription": "urn:s:1", "NGSILD-Tenant": 7}, "body": {}}),
        json!({"metadata": {"x-op": "x-dance"}}),
    ];
    for m in broken {
        assert!(check_server(&m).is_err(), "described: {m}");
    }
}

/// The client side: a message `read` accepts is one the document describes,
/// and one it refuses is one the document does not, both ways.
#[test]
fn read_and_the_document_agree_on_client_messages() {
    let long_id = "i".repeat(envelope::MAX_ID_BYTES);
    let too_long_id = "i".repeat(envelope::MAX_ID_BYTES.saturating_add(1));
    let corpus = [
        (
            true,
            "subscribe",
            json!({"metadata": {"x-op": "x-subscribe", "x-id": "1"}, "body": {"type": "Subscription"}}),
        ),
        (
            true,
            "subscribe",
            json!({"metadata": {"x-op": "x-subscribe"}, "body": {"type": "Subscription"}}),
        ),
        (
            true,
            "subscribe",
            json!({"metadata": {"x-op": "x-subscribe", "x-id": long_id}, "body": {"type": "Subscription"}}),
        ),
        (
            false,
            "subscribe",
            json!({"metadata": {"x-op": "x-subscribe", "x-id": too_long_id}, "body": {"type": "Subscription"}}),
        ),
        (
            false,
            "subscribe",
            json!({"metadata": {"x-op": "x-subscribe"}}),
        ),
        (
            false,
            "subscribe",
            json!({"metadata": {"x-op": "x-subscribe"}, "body": []}),
        ),
        (
            false,
            "subscribe",
            json!({"metadata": {"x-op": "x-subscribe", "x-other": 1}, "body": {}}),
        ),
        (
            false,
            "subscribe",
            json!({"metadata": {"x-op": "x-subscribe"}, "body": {}, "extra": 1}),
        ),
        (
            true,
            "update",
            json!({"metadata": {"x-op": "x-update", "x-subscription": "urn:s:1"}, "body": {"q": "a"}}),
        ),
        (
            false,
            "update",
            json!({"metadata": {"x-op": "x-update"}, "body": {}}),
        ),
        (
            true,
            "unsubscribe",
            json!({"metadata": {"x-op": "x-unsubscribe", "x-id": "u", "x-subscription": "urn:s:1"}}),
        ),
        (
            false,
            "unsubscribe",
            json!({"metadata": {"x-op": "x-unsubscribe"}}),
        ),
        (
            false,
            "unsubscribe",
            json!({"metadata": {"x-op": "x-unsubscribe", "x-subscription": "urn:s:1"}, "body": {}}),
        ),
        (
            true,
            "resume",
            json!({"metadata": {"x-op": "x-resume", "x-connection": CONNECTION, "x-resume-token": TOKEN, "x-seq": 0}}),
        ),
        (
            true,
            "resume",
            json!({"metadata": {"x-op": "x-resume", "x-id": "r", "x-connection": CONNECTION, "x-resume-token": TOKEN, "x-seq": 3}}),
        ),
        (
            false,
            "resume",
            json!({"metadata": {"x-op": "x-resume", "x-connection": CONNECTION, "x-resume-token": TOKEN}}),
        ),
        (
            false,
            "resume",
            json!({"metadata": {"x-op": "x-resume", "x-connection": CONNECTION, "x-resume-token": TOKEN, "x-seq": -1}}),
        ),
    ];
    for (valid, name, m) in corpus {
        let by_doc = check(name, &m);
        let by_read = read(&m.to_string());
        assert_eq!(by_doc.is_ok(), valid, "document on {m}: {by_doc:?}");
        assert_eq!(by_read.is_ok(), valid, "read on {m}: {by_read:?}");
    }
}

/// Security of T-3518: the contract states the Antares extensions as
/// extensions. Every `metadata` member a message declares is a TS 104 243
/// Table 5.2-2 key or carries the `x-` prefix, and so does every message name.
#[test]
fn every_antares_member_is_marked_as_an_extension() -> Result<(), String> {
    const TS_104_243: [&str; 2] = ["Content-Type", "Link"];
    let doc = doc()?;
    let messages = doc
        .pointer("/components/messages")
        .and_then(Value::as_object)
        .ok_or("no components.messages")?;
    for (name, m) in messages {
        let message = m.get("name").and_then(Value::as_str).unwrap_or_default();
        assert!(message.starts_with("x-"), "{name}: {message}");
        let payload = resolve(doc, m.get("payload").unwrap_or(&Value::Null));
        let members = payload
            .pointer("/properties/metadata/properties")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("{name} declares no metadata members"))?;
        for member in members.keys() {
            assert!(
                member.starts_with("x-") || TS_104_243.contains(&member.as_str()),
                "{name}: metadata.{member} is neither TS 104 243 nor x-"
            );
        }
    }
    Ok(())
}
