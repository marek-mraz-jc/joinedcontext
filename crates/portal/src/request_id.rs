//! One reference per request, the same in the browser, the edge and the Portal's log (UI-16,
//! OPS-15, T-2747).
//!
//! The edge's `request-id` plugin names every request and returns the name in `X-Request-Id`;
//! a failure page shows it, so a person can quote it and whoever reads the log finds the line.
//! The Portal takes the edge's id when it is a plain token, mints one otherwise (a Portal run
//! without the edge, or a header nobody should trust into a log), puts it on the span every log
//! line of the request is written in, and answers with it.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

pub const HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// A UUID or a similar token: letters, digits and dashes, 8 to 64 of them. Anything else could
/// forge a log line or carry more than a reference.
fn plain(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn fresh() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

tokio::task_local! {
    /// The id of the request the current task answers, for the calls it makes to the Portal's
    /// neighbours (the forge, the gateway): theirs log the same reference.
    static CURRENT: String;
}

/// `builder` with the current request's id, when the task answers a request.
pub fn forward(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match CURRENT.try_with(Clone::clone) {
        Ok(id) => builder.header(HEADER, id),
        Err(_) => builder,
    }
}

/// Runs `f` as if it answered the request `id` (a test's stand-in for [`middleware`]).
#[cfg(test)]
pub async fn answering<F: std::future::Future>(id: &str, f: F) -> F::Output {
    CURRENT.scope(id.to_owned(), f).await
}

pub async fn middleware(request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get(&HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|id| plain(id))
        .map_or_else(fresh, str::to_owned);
    let span = tracing::info_span!("request", request_id = %id);
    let mut response = CURRENT
        .scope(id.clone(), next.run(request).instrument(span.clone()))
        .await;
    if response.status().is_server_error() {
        span.in_scope(|| tracing::warn!(status = response.status().as_u16(), "request failed"));
    }
    // `plain` or hex: always a valid header value.
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(HEADER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_token_is_taken_from_the_edge() {
        assert!(plain("3f2c1a9e-6b1d-4c3e-9a8f-0d1e2f3a4b5c"));
        assert!(!plain("short"));
        assert!(!plain("a\nlevel=error msg=forged"));
        assert!(!plain(&"a".repeat(65)));
        assert!(!plain("id with spaces"));
    }

    #[tokio::test]
    async fn a_call_made_while_answering_a_request_carries_its_id() {
        let client = reqwest::Client::new();
        let outside = forward(client.get("http://127.0.0.1:9/"))
            .build()
            .expect("request");
        assert!(outside.headers().get(&HEADER).is_none());
        let inside = CURRENT
            .scope("3f2c1a9e-6b1d-4c3e".to_owned(), async {
                forward(client.get("http://127.0.0.1:9/"))
                    .build()
                    .expect("request")
            })
            .await;
        assert_eq!(
            inside.headers().get(&HEADER).map(|v| v.to_str().ok()),
            Some(Some("3f2c1a9e-6b1d-4c3e"))
        );
    }

    #[test]
    fn a_minted_id_is_plain_and_new_each_time() {
        let (one, two) = (fresh(), fresh());
        assert!(plain(&one) && one.len() == 32);
        assert_ne!(one, two);
    }
}
