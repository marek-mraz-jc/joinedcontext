//! Keeping the realm's signing keys current (T-0228, PF-46).
//!
//! The JWKS is fetched in the background and swapped in whole. A request never fetches:
//! an unknown key id is a rejection, because a token that names a key nobody published is
//! not a token, and letting a request trigger an outbound fetch is a way to make the
//! gateway do work on an attacker's schedule.
//!
//! The URL is the in-cluster Keycloak address, so plain HTTP is what the gateway speaks
//! here; the hop is inside the mesh, where Linkerd provides mTLS. An `https` JWKS URL is
//! refused at start-up rather than silently downgraded.

use crate::auth::token::Verifier;
use axum::body::Body;
use jsonwebtoken::jwk::JwkSet;
use std::sync::Arc;
use std::time::Duration;

/// How often the keys are re-fetched. Keycloak rotates on the order of days; a realm that
/// has just rotated must not lock every caller out until the next restart.
pub const REFRESH: Duration = Duration::from_secs(300);

/// Why the keys could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JwksError {
    /// The URL is not one the gateway fetches over.
    #[error("{0}")]
    Url(String),
    /// The realm did not answer, or answered with something other than a JWKS.
    #[error("{0}")]
    Unreachable(String),
}

/// How long one refresh may take, the request and its body together (F-PLT2-024): a realm that
/// accepts and never answers held the gateway's startup, and the refresh loop, for ever.
const REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Fetches the JWKS at `url` and installs its keys, returning how many were usable.
pub async fn refresh(verifier: &Verifier, url: &str) -> Result<usize, JwksError> {
    refresh_at(verifier, url, None).await
}

/// [`refresh`], dialing `url`'s host at `reach` instead of through DNS (a hosting process's
/// loopback listener that routes by the host the request names).
pub async fn refresh_at(
    verifier: &Verifier,
    url: &str,
    reach: Option<std::net::SocketAddr>,
) -> Result<usize, JwksError> {
    refresh_within(verifier, url, reach, REFRESH_TIMEOUT).await
}

async fn refresh_within(
    verifier: &Verifier,
    url: &str,
    reach: Option<std::net::SocketAddr>,
    timeout: std::time::Duration,
) -> Result<usize, JwksError> {
    if url.starts_with("https://") {
        return Err(JwksError::Url(
            "the JWKS URL must be the in-cluster http:// address; the gateway does not speak TLS to Keycloak"
                .to_owned(),
        ));
    }
    if !url.starts_with("http://") {
        return Err(JwksError::Url(
            "the JWKS URL must start with http://".to_owned(),
        ));
    }

    let uri = url.parse().map_err(|e| JwksError::Url(format!("{e}")))?;
    let bytes = tokio::time::timeout(timeout, fetch(uri, reach))
        .await
        .map_err(|_| {
            JwksError::Unreachable(format!("the realm did not answer within {timeout:?}"))
        })??;
    let jwks: JwkSet =
        serde_json::from_slice(&bytes).map_err(|e| JwksError::Unreachable(e.to_string()))?;

    Ok(verifier.replace_keys(&jwks))
}

/// The JWKS document's bytes, at most 1 MiB.
async fn fetch(
    uri: axum::http::Uri,
    reach: Option<std::net::SocketAddr>,
) -> Result<axum::body::Bytes, JwksError> {
    let client: hyper_util::client::legacy::Client<_, Body> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build_http();
    let mut request = axum::http::Request::get(&uri);
    let uri = match (reach, uri.authority()) {
        // The request names the URL's host; the connection goes to `reach`.
        (Some(addr), Some(authority)) => {
            request = request.header(axum::http::header::HOST, authority.as_str());
            let path = uri.path_and_query().map_or("/", |p| p.as_str());
            format!("http://{addr}{path}")
                .parse()
                .map_err(|e| JwksError::Url(format!("{e}")))?
        }
        _ => uri,
    };
    let request = request
        .uri(uri)
        .body(Body::empty())
        .map_err(|e| JwksError::Url(e.to_string()))?;
    let response = client
        .request(request)
        .await
        .map_err(|e| JwksError::Unreachable(e.to_string()))?;

    if !response.status().is_success() {
        return Err(JwksError::Unreachable(format!(
            "the realm answered {}",
            response.status()
        )));
    }
    axum::body::to_bytes(Body::new(response.into_body()), 1024 * 1024)
        .await
        .map_err(|e| JwksError::Unreachable(e.to_string()))
}

/// Refreshes the keys forever, starting now.
///
/// A failed refresh keeps the keys the verifier already has: a realm that is briefly
/// unreachable must not take every authenticated caller down with it.
pub async fn keep_current(verifier: Arc<Verifier>, url: String) {
    keep_current_at(verifier, url, None).await;
}

/// [`keep_current`] with [`refresh_at`]'s `reach`.
pub async fn keep_current_at(
    verifier: Arc<Verifier>,
    url: String,
    reach: Option<std::net::SocketAddr>,
) {
    loop {
        match refresh_at(&verifier, &url, reach).await {
            Ok(count) => tracing::info!(count, "realm signing keys loaded"),
            Err(error) => {
                tracing::error!(%error, url = %url, "cannot refresh the realm signing keys")
            }
        }
        tokio::time::sleep(REFRESH).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reached JWKS URL is fetched on `reach` and names its own host, so a listener routing
    /// by host (jc in host mode) answers it, never DNS.
    #[tokio::test]
    async fn a_reached_url_dials_the_reach_and_names_its_host() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a listener");
        let address = listener.local_addr().expect("an address");
        let served = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("a connection");
            let mut head = vec![0; 4096];
            let n = socket.read(&mut head).await.expect("a request");
            let body = r#"{"keys":[]}"#;
            let answer = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(answer.as_bytes())
                .await
                .expect("an answer");
            String::from_utf8_lossy(&head[..n]).to_ascii_lowercase()
        });
        let verifier = Verifier::new("https://idm.city.invalid/realms/r");
        refresh_at(
            &verifier,
            "http://idm.city.invalid/realms/r/protocol/openid-connect/certs",
            Some(address),
        )
        .await
        .expect("the keys from the reach");
        let head = served.await.expect("the request");
        assert!(
            head.starts_with("get /realms/r/protocol/openid-connect/certs http/1.1"),
            "{head}"
        );
        assert!(head.contains("\r\nhost: idm.city.invalid\r\n"), "{head}");
    }

    /// F-PLT2-024: a realm that accepts the connection and never answers fails the refresh
    /// within its timeout instead of holding the caller for ever.
    #[tokio::test]
    async fn a_realm_that_never_answers_fails_within_the_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a listener");
        let address = listener.local_addr().expect("an address");
        let held = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.expect("a connection");
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        let verifier = Verifier::new("http://realm");
        let started = std::time::Instant::now();
        let outcome = refresh_within(
            &verifier,
            &format!("http://{address}/certs"),
            None,
            std::time::Duration::from_millis(300),
        )
        .await;
        assert!(
            matches!(outcome, Err(JwksError::Unreachable(_))),
            "{outcome:?}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        held.abort();
    }
}
