//! The JSON-RPC envelope of the gateway's own MCP answers: the hub, the named servers and the
//! two describe tools (T-3648). Every data tool is the broker's (plan 5.3).

use axum::body::Body;
use axum::http::{Response, StatusCode};
use serde_json::{json, Value};

/// The revision of the MCP specification the hub, the named servers and the gateway's own
/// answers speak.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// A tool result an agent can both read and parse: the structured half is an object matching
/// the tool's output schema, never a bare list, which a client reading `structuredContent` by
/// name would read as nothing (T-0946).
pub(crate) fn answered(payload: &Value, result_key: &str) -> Value {
    json!({
        "isError": false,
        "content": [{
            "type": "text",
            "text": payload.to_string(),
        }],
        "structuredContent": { result_key: payload },
    })
}

/// A tool error: what was refused, in the agent's own channel for it.
///
/// The problem document rides along when there is one; nothing does when the refusal is the
/// gateway's own words, because `structuredContent` is an object or it is absent.
pub(crate) fn refused(text: &str, payload: &Value) -> Value {
    let mut result = json!({
        "isError": true,
        "content": [{ "type": "text", "text": text }],
    });
    if let Value::Object(document) = payload {
        result["structuredContent"] = Value::Object(document.clone());
    }
    result
}

/// A JSON-RPC result.
pub(crate) fn result(id: Value, value: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": value })
}

/// A JSON-RPC error.
pub(crate) fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The JSON-RPC parse error, as an HTTP answer.
pub fn parse_error() -> Response<Body> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": Value::Null,
        "error": { "code": -32700, "message": "parse error" },
    });
    json_response(StatusCode::OK, &body)
}

/// A JSON-RPC message as the HTTP answer Streamable HTTP expects.
pub fn json_response(status: StatusCode, body: &Value) -> Response<Body> {
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refusal carries the problem document as `structuredContent` only when it is an object.
    #[test]
    fn a_refusal_carries_only_an_object_as_its_structured_content() {
        let with = refused("refused", &json!({"status": 403}));
        assert_eq!(with["isError"], json!(true));
        assert_eq!(with["structuredContent"], json!({"status": 403}));
        assert!(refused("refused", &Value::Null)
            .get("structuredContent")
            .is_none());
        assert!(refused("refused", &json!([1]))
            .get("structuredContent")
            .is_none());
        let ok = answered(&json!([1, 2]), "items");
        assert_eq!(ok["content"][0]["text"], json!("[1,2]"));
        assert_eq!(ok["structuredContent"]["items"], json!([1, 2]));
        let answer = json_response(StatusCode::ACCEPTED, &json!({"a": 1}));
        assert_eq!(answer.status(), StatusCode::ACCEPTED);
        assert_eq!(answer.headers()["content-type"], "application/json");
    }
}
