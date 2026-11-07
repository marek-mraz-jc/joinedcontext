//! Reading what a `capture` broker kept through the registry (ADR-N-048 §2.10, T-6290).
//!
//! The route's own function answers, so the gate is the route's: only a caller who may approve
//! MqttBrokers in the project reads a capture, anybody else gets the `404` of a broker that is
//! not there (PF-59). A caller whose answers go to a model outside the Portal (an MCP client, an
//! agent run) does not see the fields the broker's DataSources list in `personalFields` (AG-158).

use std::collections::BTreeSet;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::bounds::{text, NAME};
use super::runs::as_user;
use super::{parse_input, Caller, Operation, OperationAnnotations, Via};
use crate::api::mqtt_captured::{Captured, CapturedQuery};
use crate::change::Lane;
use crate::state::AppState;

pub const CAPTURED: &str = "jc_mqtt_captured";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CapturedInput {
    pub name: String,
    pub topic: Option<String>,
    pub limit: Option<usize>,
}

fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": text("The MqttBroker's name", NAME),
            "topic": text("One exact topic, no + or #", 65_535),
            "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Default 100" }
        },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "broker": { "type": "string" },
            "mode": { "type": "string", "enum": ["ingest", "capture", "relay"] },
            "window": { "type": "string" },
            "messagesPerTopic": { "type": "integer" },
            "messages": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "topic": { "type": "string" },
                        "receivedAt": { "type": "string", "format": "date-time" },
                        "clientId": { "type": "string" },
                        "bytes": { "type": "integer" },
                        "payload": { "type": "string" },
                        "encoding": { "type": "string", "enum": ["utf-8", "base64"] }
                    },
                    "required": ["topic", "receivedAt", "bytes", "payload", "encoding"]
                }
            },
            "truncated": { "type": "boolean" },
            "withheld": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Each personal field left out, as DataSource/{name}.{field}; a payload that is no JSON object is left out whole"
            }
        },
        "required": ["broker", "mode", "messages", "truncated"]
    })
}

/// Each DataSource of `project` reading broker `broker` that declares `spec.personalFields`, with
/// the fields it lists (an empty list declares none personal).
fn personal_fields(
    state: &AppState,
    project: &str,
    broker: &str,
) -> Vec<(String, BTreeSet<String>)> {
    state
        .mirror
        .kind_where(project, "DataSource", |source| {
            source.spec.get("personalFields").is_some()
                && source
                    .spec
                    .pointer("/mqtt/brokerRef/name")
                    .and_then(Value::as_str)
                    == Some(broker)
        })
        .into_iter()
        .map(|source| {
            let fields = crate::personal_data::source_fields(state, project, &source.metadata.name);
            (source.metadata.name, fields)
        })
        .collect()
}

/// `captured` as a model outside the Portal may read it (AG-158, ADR-N-048 item 10), deny by
/// default: no `clientId`, and no payload unless a DataSource reading the broker declares its
/// personal fields. Those fields leave each payload that is a JSON object, and a payload that is
/// not one leaves whole, since a text cannot lose one field. Returns what was left out, a field
/// as `DataSource/{source}.{field}`; values are never returned.
fn withhold(captured: &mut Captured, declared: &[(String, BTreeSet<String>)]) -> Vec<String> {
    let mut removed = BTreeSet::new();
    for message in &mut captured.messages {
        if message.client_id.take().is_some() {
            removed.insert("clientId".to_owned());
        }
        if declared.is_empty() {
            message.payload = String::new();
            removed.insert(format!(
                "payload: no DataSource reading broker {} lists its personal fields",
                captured.broker
            ));
            continue;
        }
        let parsed = (message.encoding == "utf-8")
            .then(|| serde_json::from_str::<Value>(&message.payload).ok())
            .flatten();
        match parsed {
            Some(Value::Object(mut members)) => {
                let mut changed = false;
                for (source, fields) in declared {
                    for field in fields {
                        if members.remove(field).is_some() {
                            removed.insert(format!("DataSource/{source}.{field}"));
                            changed = true;
                        }
                    }
                }
                if changed {
                    message.payload = Value::Object(members).to_string();
                }
            }
            _ if declared.iter().any(|(_, fields)| !fields.is_empty()) => {
                message.payload = String::new();
                for (source, fields) in declared {
                    removed.extend(fields.iter().map(|f| format!("DataSource/{source}.{f}")));
                }
            }
            _ => {}
        }
    }
    removed.into_iter().collect()
}

async fn captured(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: CapturedInput,
) -> Result<Value, super::OpError> {
    let name = input.name.clone();
    let Json(mut answer) = crate::api::mqtt_captured::get_captured(
        as_user(caller),
        State(state.clone()),
        Path((project.to_owned(), input.name)),
        Query(CapturedQuery {
            topic: input.topic,
            limit: input.limit,
        }),
    )
    .await?;
    let withheld = if matches!(caller.via, Via::Mcp | Via::Agent) {
        withhold(&mut answer, &personal_fields(state, project, &name))
    } else {
        Vec::new()
    };
    let mut value = serde_json::to_value(answer)?;
    if !withheld.is_empty() {
        value["withheld"] = json!(withheld);
    }
    Ok(value)
}

/// `jc_mqtt_test_publish`: the broker card's Test drawer, a dry run of one device message (JC2-59).
pub const TEST_PUBLISH: &str = "jc_mqtt_test_publish";
/// The largest payload the route takes.
const MAX_PAYLOAD: u64 = 1024 * 1024;

/// `name` is the broker, beside the route's own body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestInput {
    name: String,
    account: String,
    topic: String,
    payload: String,
}

fn test_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": text("MqttBroker name, e.g. sensors", NAME),
            "account": text("A ServiceAccount the broker's roles name, e.g. cam-west", NAME),
            "topic": text("No wildcard, e.g. cameras/cam-west/frame", 1024),
            "payload": { "type": "string", "maxLength": MAX_PAYLOAD, "description": "The message as the device sends it, e.g. {\"temperature\": 21.5}" }
        },
        "required": ["name", "account", "topic", "payload"],
        "additionalProperties": false
    })
}

fn test_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "Each step's outcome (ACL, limits, DataSource and Pipeline, mapping, record check); nothing published or written"
    })
}

async fn test_publish(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: TestInput,
) -> Result<Value, super::OpError> {
    let Json(dry_run) = crate::api::mqtt_test_publish::test_publish(
        as_user(caller),
        State(state.clone()),
        Path((project.to_owned(), input.name)),
        Json(crate::api::mqtt_test_publish::TestPublish {
            account: input.account,
            topic: input.topic,
            payload: input.payload,
        }),
    )
    .await?;
    Ok(serde_json::to_value(dry_run)?)
}

pub fn operations() -> Vec<Operation> {
    vec![Operation {
        name: TEST_PUBLISH,
        title: "Test an MQTT Publish",
        description: "Dry-runs one device message on a topic through the broker's ACL and limits, the bound DataSource and Pipeline, the mapping and the Endpoint's record check; publishes and writes nothing",
        input: test_input_schema,
        output: test_output_schema,
        annotations: OperationAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
        },
        kind: "MqttBroker",
        verb: None,
        lane: Lane::Green,
        validate: |val| parse_input::<TestInput>(val.clone()).map(|_| ()),
        run: |caller, state, project, val| {
            Box::pin(async move { test_publish(caller, state, project, parse_input(val)?).await })
        },
    }, Operation {
        name: CAPTURED,
        title: "Read An MQTT Broker's Capture",
        description:
            "The newest messages a capture broker kept; 404 to whoever may not approve MqttBrokers",
        input: input_schema,
        output: output_schema,
        annotations: OperationAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
        },
        kind: "MqttBroker",
        verb: None,
        lane: Lane::Green,
        validate: |val| parse_input::<CapturedInput>(val.clone()).map(|_| ()),
        run: |caller, state, project, val| {
            Box::pin(async move { captured(caller, state, project, parse_input(val)?).await })
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::mqtt_captured::CapturedMessage;
    use chrono::DateTime;

    fn message(payload: &str, encoding: &'static str) -> CapturedMessage {
        CapturedMessage {
            topic: "sensors/s-12/air".to_owned(),
            received_at: DateTime::from_timestamp(0, 0).unwrap_or_default(),
            client_id: None,
            bytes: payload.len(),
            payload: payload.to_owned(),
            encoding,
        }
    }

    #[test]
    fn a_personal_field_leaves_a_json_payload_and_a_text_payload_leaves_whole() {
        let mut captured = Captured {
            broker: "sensors".to_owned(),
            mode: "capture".to_owned(),
            window: None,
            messages_per_topic: None,
            messages: vec![
                message(r#"{"pm10": 12, "owner": "Jana"}"#, "utf-8"),
                message("pm10=12;owner=Jana", "utf-8"),
                message("/wAB", "base64"),
            ],
            truncated: false,
        };
        let personal = vec![("air".to_owned(), BTreeSet::from(["owner".to_owned()]))];
        let removed = withhold(&mut captured, &personal);
        assert_eq!(removed, vec!["DataSource/air.owner".to_owned()]);
        let payloads: Vec<&str> = captured
            .messages
            .iter()
            .map(|m| m.payload.as_str())
            .collect();
        assert_eq!(payloads, vec![r#"{"pm10":12}"#, "", ""]);
    }

    #[test]
    fn a_broker_no_source_declares_keeps_every_payload_and_client_id_back() {
        let mut held = message(r#"{"pm10": 12}"#, "utf-8");
        held.client_id = Some("s-12".to_owned());
        let mut captured = Captured {
            broker: "sensors".to_owned(),
            mode: "capture".to_owned(),
            window: None,
            messages_per_topic: None,
            messages: vec![held],
            truncated: false,
        };
        let removed = withhold(&mut captured, &[]);
        assert_eq!(
            removed,
            vec![
                "clientId".to_owned(),
                "payload: no DataSource reading broker sensors lists its personal fields"
                    .to_owned()
            ]
        );
        let kept = captured
            .messages
            .first()
            .map(|m| (m.payload.as_str(), m.client_id.clone(), m.bytes));
        assert_eq!(kept, Some(("", None, 12)));
    }

    #[test]
    fn a_source_declaring_no_personal_field_keeps_the_payload() {
        let mut captured = Captured {
            broker: "sensors".to_owned(),
            mode: "capture".to_owned(),
            window: None,
            messages_per_topic: None,
            messages: vec![message("pm10=12", "utf-8")],
            truncated: false,
        };
        let declared = vec![("air".to_owned(), BTreeSet::new())];
        assert!(withhold(&mut captured, &declared).is_empty());
        assert_eq!(
            captured.messages.first().map(|m| m.payload.as_str()),
            Some("pm10=12")
        );
    }
}
