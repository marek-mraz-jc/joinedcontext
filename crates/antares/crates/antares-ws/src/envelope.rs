// SPDX-License-Identifier: EUPL-1.2
//! The TS 104 243 envelope in both directions (ADR-0026 rules 3 and 5).
//!
//! Every message is one text frame holding `{"metadata": {...}, "body": ...}`.
//! A client message is a closed set: its `metadata` names one `x-op` and
//! nothing the server does not read (`deny_unknown_fields`, threat W11), and
//! its bounds (size, JSON depth) are checked before `serde` runs (W3).

use antares_ops::bounds::{json_depth, MAX_JSON_DEPTH};
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// The largest client message accepted (ADR-0026 rule 10): a Subscription
/// is small; the 4 MiB body cap is for Entity writes.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// The largest WebSocket frame accepted (ADR-0026 rule 10).
pub const MAX_FRAME_BYTES: usize = 16 * 1024;

/// The longest `x-id` echoed back: a correlation id, not a payload.
pub const MAX_ID_BYTES: usize = 256;

/// One client message, read.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientMessage {
    /// `x-id`, echoed on the answer.
    pub id: Option<String>,
    /// What the client asks.
    pub op: ClientOp,
}

/// What a client message asks (ADR-0026 rule 5, client side).
#[derive(Debug, Clone, PartialEq)]
pub enum ClientOp {
    /// Create Subscription (10.5.2) with this connection as its endpoint.
    Subscribe(Map<String, Value>),
    /// Update Subscription (10.5.3) of a Subscription this connection made.
    Update {
        /// `x-subscription`: the Subscription's id.
        subscription: String,
        /// The fragment.
        fragment: Map<String, Value>,
    },
    /// Delete Subscription of a Subscription this connection made.
    Unsubscribe {
        /// `x-subscription`: the Subscription's id.
        subscription: String,
    },
    /// Take over a connection within its grace period (rule 7).
    Resume {
        /// `x-connection`.
        connection: String,
        /// `x-resume-token`.
        token: String,
        /// `x-seq`: the last notification the client processed.
        seq: u64,
    },
}

/// Why a client message was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unread {
    /// Past a bound of rule 10: the connection closes.
    Close(&'static str),
    /// Not a message of rule 3: answered with `BadRequestData`, the
    /// connection stays.
    Bad(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    metadata: Metadata,
    #[serde(default)]
    body: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    #[serde(rename = "x-op")]
    op: OpName,
    #[serde(rename = "x-id", default)]
    id: Option<String>,
    #[serde(rename = "x-subscription", default)]
    subscription: Option<String>,
    #[serde(rename = "x-connection", default)]
    connection: Option<String>,
    #[serde(rename = "x-resume-token", default)]
    token: Option<String>,
    #[serde(rename = "x-seq", default)]
    seq: Option<u64>,
}

#[derive(Deserialize, Clone, Copy)]
enum OpName {
    #[serde(rename = "x-subscribe")]
    Subscribe,
    #[serde(rename = "x-update")]
    Update,
    #[serde(rename = "x-unsubscribe")]
    Unsubscribe,
    #[serde(rename = "x-resume")]
    Resume,
}

/// Read one client text message.
///
/// ```
/// use antares_ws::envelope::{read, ClientOp};
///
/// let m = read(r#"{"metadata":{"x-op":"x-unsubscribe","x-subscription":"urn:s:1"}}"#).unwrap();
/// assert_eq!(m.op, ClientOp::Unsubscribe { subscription: "urn:s:1".into() });
/// ```
///
/// # Errors
/// [`Unread::Close`] past [`MAX_MESSAGE_BYTES`] or [`MAX_JSON_DEPTH`],
/// [`Unread::Bad`] for anything else that is not a message of rule 3.
pub fn read(text: &str) -> Result<ClientMessage, Unread> {
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(Unread::Close("message too large"));
    }
    if json_depth(text.as_bytes()) > MAX_JSON_DEPTH {
        return Err(Unread::Close("message nested too deep"));
    }
    let env: Envelope = serde_json::from_str(text).map_err(|e| Unread::Bad(e.to_string()))?;
    let m = env.metadata;
    if m.id.as_ref().is_some_and(|i| i.len() > MAX_ID_BYTES) {
        return Err(Unread::Bad(format!(
            "x-id is longer than {MAX_ID_BYTES} bytes"
        )));
    }
    let object = |body: Option<Value>, what: &str| match body {
        Some(Value::Object(o)) => Ok(o),
        _ => Err(Unread::Bad(format!("{what} carries a JSON object as body"))),
    };
    let absent = |body: &Option<Value>, what: &str| match body {
        None => Ok(()),
        Some(_) => Err(Unread::Bad(format!("{what} carries no body"))),
    };
    let named = |v: Option<String>, member: &str| {
        v.ok_or_else(|| Unread::Bad(format!("metadata.{member} is required")))
    };
    let op = match m.op {
        OpName::Subscribe => ClientOp::Subscribe(object(env.body, "x-subscribe")?),
        OpName::Update => ClientOp::Update {
            subscription: named(m.subscription, "x-subscription")?,
            fragment: object(env.body, "x-update")?,
        },
        OpName::Unsubscribe => {
            absent(&env.body, "x-unsubscribe")?;
            ClientOp::Unsubscribe {
                subscription: named(m.subscription, "x-subscription")?,
            }
        }
        OpName::Resume => {
            absent(&env.body, "x-resume")?;
            ClientOp::Resume {
                connection: named(m.connection, "x-connection")?,
                token: named(m.token, "x-resume-token")?,
                seq: m
                    .seq
                    .ok_or_else(|| Unread::Bad("metadata.x-seq is required".into()))?,
            }
        }
    };
    Ok(ClientMessage { id: m.id, op })
}

fn message(metadata: Map<String, Value>, body: Option<Value>) -> String {
    let mut m = Map::new();
    m.insert("metadata".into(), Value::Object(metadata));
    if let Some(body) = body {
        m.insert("body".into(), body);
    }
    Value::Object(m).to_string()
}

fn meta(op: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("x-op".into(), Value::String(op.into()));
    m
}

/// `x-welcome`: the connection id and the resume token (rule 4, 7).
#[must_use]
pub fn welcome(connection: &str, token: &str) -> String {
    let mut m = meta("x-welcome");
    m.insert("x-connection".into(), connection.into());
    m.insert("x-resume-token".into(), token.into());
    message(m, None)
}

/// `x-result`: the answer to a client message, with the HTTP status the
/// same request over HTTP would have answered.
#[must_use]
pub fn result(id: Option<&str>, status: u16, body: Option<Value>) -> String {
    let mut m = meta("x-result");
    if let Some(id) = id {
        m.insert("x-id".into(), id.into());
    }
    m.insert("x-status".into(), status.into());
    message(m, body)
}

/// A `ProblemDetails` (TS 104 175 5.5.3) of type `BadRequestData`.
#[must_use]
pub fn bad_request(detail: &str) -> Value {
    json!({
        "type": "https://uri.etsi.org/ngsi-ld/errors/BadRequestData",
        "title": "BadRequestData",
        "status": 400,
        "detail": detail,
    })
}

/// A `ProblemDetails` of type `ResourceNotFound`, the one answer to a
/// failed resume and to a Subscription this connection did not make.
#[must_use]
pub fn not_found() -> Value {
    json!({
        "type": "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound",
        "title": "ResourceNotFound",
        "status": 404,
    })
}

/// `x-notification`: the TS 104 243 message the MQTT binding would publish
/// (`rendered`), with `x-seq` and `x-subscription` added to its `metadata`,
/// and `x-coalesced` when it replaced that many queued ones of the same
/// Entity (back-pressure, ADR-0026 rule 6).
#[must_use]
pub fn notification(rendered: &Value, seq: u64, subscription: &str, coalesced: u64) -> String {
    let mut m = rendered
        .get("metadata")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    m.insert("x-op".into(), "x-notification".into());
    m.insert("x-seq".into(), seq.into());
    m.insert("x-subscription".into(), subscription.into());
    if coalesced > 0 {
        m.insert("x-coalesced".into(), coalesced.into());
    }
    message(m, rendered.get("body").cloned())
}

/// `x-gap`: notifications `from` to `to` are lost.
#[must_use]
pub fn gap(from: u64, to: u64) -> String {
    let mut m = meta("x-gap");
    m.insert("x-from".into(), from.into());
    m.insert("x-to".into(), to.into());
    message(m, None)
}

/// `x-closing`, sent before the server closes.
#[must_use]
pub fn closing(reason: &str) -> String {
    let mut m = meta("x-closing");
    m.insert("x-reason".into(), reason.into());
    message(m, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bad(text: &str) -> bool {
        matches!(read(text), Err(Unread::Bad(_)))
    }

    #[test]
    fn every_client_op_reads() -> Result<(), Unread> {
        let sub = read(
            r#"{"metadata":{"x-op":"x-subscribe","x-id":"1"},"body":{"type":"Subscription"}}"#,
        )?;
        assert_eq!(sub.id.as_deref(), Some("1"));
        let mut body = Map::new();
        body.insert("type".into(), Value::String("Subscription".into()));
        assert_eq!(sub.op, ClientOp::Subscribe(body));
        let up = read(r#"{"metadata":{"x-op":"x-update","x-subscription":"s"},"body":{"q":"a"}}"#)?;
        let mut fragment = Map::new();
        fragment.insert("q".into(), Value::String("a".into()));
        assert_eq!(
            up.op,
            ClientOp::Update {
                subscription: "s".into(),
                fragment
            }
        );
        let resume = read(
            r#"{"metadata":{"x-op":"x-resume","x-connection":"c","x-resume-token":"t","x-seq":7}}"#,
        )?;
        assert_eq!(
            resume.op,
            ClientOp::Resume {
                connection: "c".into(),
                token: "t".into(),
                seq: 7
            }
        );
        Ok(())
    }

    #[test]
    fn what_is_not_a_message_is_bad_data_and_does_not_close() {
        for text in [
            "",
            "[]",
            "null",
            r#"{"metadata":{"x-op":"x-dance"}}"#,
            r#"{"metadata":{"x-op":"x-subscribe"}}"#,
            r#"{"metadata":{"x-op":"x-subscribe"},"body":[]}"#,
            r#"{"metadata":{"x-op":"x-subscribe"},"body":{},"extra":1}"#,
            r#"{"metadata":{"x-op":"x-subscribe","x-other":1},"body":{}}"#,
            r#"{"metadata":{"x-op":"x-update"},"body":{}}"#,
            r#"{"metadata":{"x-op":"x-unsubscribe"}}"#,
            r#"{"metadata":{"x-op":"x-unsubscribe","x-subscription":"s"},"body":{}}"#,
            r#"{"metadata":{"x-op":"x-resume","x-connection":"c","x-resume-token":"t"}}"#,
            r#"{"metadata":{"x-op":"x-resume","x-connection":"c","x-seq":1}}"#,
            r#"{"metadata":{"x-op":"x-resume","x-resume-token":"t","x-seq":1}}"#,
            r#"{"metadata":{"x-op":"x-resume","x-connection":"c","x-resume-token":"t","x-seq":-1}}"#,
            r#"{"metadata":{"x-op":"x-resume","x-connection":"c","x-resume-token":"t","x-seq":1},"body":1}"#,
            r#"{"body":{}}"#,
        ] {
            assert!(bad(text), "{text}");
        }
        let long = format!(
            r#"{{"metadata":{{"x-op":"x-unsubscribe","x-subscription":"s","x-id":"{}"}}}}"#,
            "i".repeat(MAX_ID_BYTES + 1)
        );
        assert!(bad(&long));
    }

    #[test]
    fn past_a_bound_the_connection_closes() {
        let deep = format!(
            r#"{{"metadata":{{"x-op":"x-subscribe"}},"body":{}{}}}"#,
            "[".repeat(MAX_JSON_DEPTH),
            "]".repeat(MAX_JSON_DEPTH)
        );
        assert_eq!(read(&deep), Err(Unread::Close("message nested too deep")));
        let big = format!(
            r#"{{"metadata":{{"x-op":"x-subscribe"}},"body":{{"d":"{}"}}}}"#,
            "x".repeat(MAX_MESSAGE_BYTES)
        );
        assert_eq!(read(&big), Err(Unread::Close("message too large")));
        // at the depth bound it is read
        let at = format!(
            r#"{{"metadata":{{"x-op":"x-subscribe"}},"body":{{"a":{}{}}}}}"#,
            "[".repeat(MAX_JSON_DEPTH - 2),
            "]".repeat(MAX_JSON_DEPTH - 2)
        );
        assert!(read(&at).is_ok());
    }

    #[test]
    fn server_messages_carry_their_members() -> Result<(), serde_json::Error> {
        let read = |text: String| serde_json::from_str::<Value>(&text);
        assert_eq!(
            read(welcome("c", "t"))?,
            json!({"metadata": {"x-op": "x-welcome", "x-connection": "c", "x-resume-token": "t"}})
        );
        assert_eq!(
            read(result(Some("9"), 404, Some(not_found())))?,
            json!({"metadata": {"x-op": "x-result", "x-id": "9", "x-status": 404}, "body": not_found()})
        );
        assert_eq!(
            read(result(None, 204, None))?,
            json!({"metadata": {"x-op": "x-result", "x-status": 204}})
        );
        let rendered =
            json!({"metadata": {"Content-Type": "application/json"}, "body": {"id": "n"}});
        assert_eq!(
            read(notification(&rendered, 3, "s", 0))?,
            json!({"metadata": {"Content-Type": "application/json", "x-op": "x-notification", "x-seq": 3, "x-subscription": "s"}, "body": {"id": "n"}})
        );
        assert_eq!(
            read(gap(2, 5))?,
            json!({"metadata": {"x-op": "x-gap", "x-from": 2, "x-to": 5}})
        );
        assert_eq!(
            read(closing("lifetime"))?,
            json!({"metadata": {"x-op": "x-closing", "x-reason": "lifetime"}})
        );
        assert_eq!(bad_request("d").get("detail"), Some(&json!("d")));
        Ok(())
    }
}
