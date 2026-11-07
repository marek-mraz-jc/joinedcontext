//! The answers the Portal reads from the services it calls (brokers, the gateway, the
//! assistant, the UI bundle), read to a cap (T-4965): a service that answers without end costs
//! one refused read, not the Portal's memory.

use jc_core::outbound::{read_capped, BodyError};
use serde::de::DeserializeOwned;

/// One answer of a broker, the gateway or the assistant: a page of 1,000 entities or an
/// entity's newest 1,000 versions per Attribute fit well below it.
pub(crate) const ANSWER_BYTES: usize = 64 * 1024 * 1024;

/// The body of `response`, refused past `max` bytes; the error names no URL.
pub(crate) async fn bytes(response: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    read_capped(response, max).await.map_err(|err| match err {
        BodyError::Transport(err) => err.without_url().to_string(),
        too_large => too_large.to_string(),
    })
}

/// The body of `response` as JSON, refused past `max` bytes.
pub(crate) async fn json<T: DeserializeOwned>(
    response: reqwest::Response,
    max: usize,
) -> Result<T, String> {
    serde_json::from_slice(&bytes(response, max).await?).map_err(|err| err.to_string())
}

/// The body of `response` as text, refused past `max` bytes.
pub(crate) async fn text(response: reqwest::Response, max: usize) -> Result<String, String> {
    Ok(String::from_utf8_lossy(&bytes(response, max).await?).into_owned())
}

/// `what: status: sentence` of a refused call, for a log line or a status condition: a problem
/// document's `detail` (or `title`), else the body, on one line, `token` redacted, cut to 300
/// bytes.
pub(crate) async fn refusal(
    what: &str,
    response: reqwest::Response,
    token: Option<&str>,
) -> String {
    let status = response.status();
    let body = text(response, ANSWER_BYTES).await.unwrap_or_default();
    // A problem document says it in `detail` (or `title`): that sentence, not its JSON (T-4866).
    let sentence = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|problem| {
            ["detail", "title"].iter().find_map(|key| {
                problem
                    .get(*key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
        });
    let mut said = sentence.unwrap_or(body).trim().replace('\n', " ");
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        said = said.replace(token, "[redacted]");
    }
    said.truncate(said.floor_char_boundary(300));
    match said.is_empty() {
        true => format!("{what}: {status}"),
        false => format!("{what}: {status}: {said}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// An answer of `body`, sent chunked so no Content-Length warns the reader ahead.
    async fn answer(body: &'static str) -> reqwest::Response {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let at = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
        });
        reqwest::get(format!("http://{at}/")).await.expect("answer")
    }

    #[tokio::test]
    async fn an_answer_within_the_cap_is_read() {
        let read: serde_json::Value = json(answer(r#"{"id":"x"}"#).await, 64).await.expect("read");
        assert_eq!(read["id"], "x");
        assert_eq!(text(answer("a,b").await, 3).await.expect("text"), "a,b");
    }

    #[tokio::test]
    async fn an_answer_past_the_cap_is_refused() {
        let refused = json::<serde_json::Value>(answer(r#"{"id":"x"}"#).await, 4).await;
        assert_eq!(
            refused.expect_err("refused"),
            "the answer is longer than 4 bytes"
        );
    }
}
