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
        answer_with("200 OK", body).await
    }

    async fn answer_with(status: &'static str, body: &'static str) -> reqwest::Response {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let at = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let reply = match body.is_empty() {
                true => format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                false => format!(
                    "HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
                    body.len()
                ),
            };
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

    #[tokio::test]
    async fn an_answer_that_is_not_json_is_refused_with_the_parse_error() {
        let refused = json::<serde_json::Value>(answer("a,b").await, 64).await;
        assert!(refused.expect_err("refused").contains("expected value"));
    }

    #[tokio::test]
    async fn a_body_cut_short_is_a_transport_error_without_the_url() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let at = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let reply = "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort";
            let _ = socket.write_all(reply.as_bytes()).await;
        });
        let response = reqwest::get(format!("http://{at}/")).await.expect("head");
        let refused = bytes(response, 1024).await.expect_err("cut short");
        assert!(!refused.contains(&at.to_string()), "{refused}");
        assert!(!refused.is_empty());
    }

    #[tokio::test]
    async fn a_refusal_quotes_the_problem_detail_then_the_title_then_the_body() {
        let detail = r#"{"title":"Forbidden","detail":"no grant on space a"}"#;
        assert_eq!(
            refusal("listing", answer_with("403 Forbidden", detail).await, None).await,
            "listing: 403 Forbidden: no grant on space a"
        );
        let title = r#"{"title":"Not Found"}"#;
        assert_eq!(
            refusal("reading", answer_with("404 Not Found", title).await, None).await,
            "reading: 404 Not Found: Not Found"
        );
        assert_eq!(
            refusal(
                "writing",
                answer_with("502 Bad Gateway", " upstream\ndown ").await,
                None
            )
            .await,
            "writing: 502 Bad Gateway: upstream down"
        );
    }

    #[tokio::test]
    async fn a_refusal_with_no_body_names_only_the_status() {
        assert_eq!(
            refusal(
                "deleting",
                answer_with("500 Internal Server Error", "").await,
                Some("")
            )
            .await,
            "deleting: 500 Internal Server Error"
        );
    }

    #[tokio::test]
    async fn a_refusal_redacts_the_token_and_stops_at_300_bytes() {
        let echoed = refusal(
            "login",
            answer_with("401 Unauthorized", "bad token tok-123 given").await,
            Some("tok-123"),
        )
        .await;
        assert_eq!(
            echoed,
            "login: 401 Unauthorized: bad token [redacted] given"
        );
        let long: &'static str = Box::leak(format!("a{}", "é".repeat(400)).into_boxed_str());
        let cut = refusal("x", answer_with("400 Bad Request", long).await, None).await;
        let said = cut.strip_prefix("x: 400 Bad Request: ").expect("prefix");
        // Byte 300 falls inside an "é": the cut stops before it rather than split the letter.
        assert_eq!(said.len(), 299);
    }
}
