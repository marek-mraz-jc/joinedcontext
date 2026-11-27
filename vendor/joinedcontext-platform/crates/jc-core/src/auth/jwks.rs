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
    if let (Some(addr), Some(authority)) = (reach, uri.authority()) {
        // The request names the URL's host; the connection goes to `reach`. The builder parses
        // the new target and reports a bad one from `body` below.
        let path = uri.path_and_query().map_or("/", |p| p.as_str());
        request = request
            .header(axum::http::header::HOST, authority.as_str())
            .uri(format!("http://{addr}{path}"));
    }
    // Every part is a parsed `Uri`'s or a socket address's own text, so the request always
    // builds; were it not, it is the URL that is bad.
    let request = request
        .body(Body::empty())
        .ok()
        .ok_or(JwksError::Url("the JWKS URL names no request".to_owned()))?;
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

/// Refreshes the keys forever, starting now, dialing [`refresh_at`]'s `reach` when given.
///
/// A failed refresh keeps the keys the verifier already has: a realm that is briefly
/// unreachable must not take every authenticated caller down with it.
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
            // held open past the refresh's timeout below, never answered
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;
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
        assert!(started.elapsed() < std::time::Duration::from_millis(900));
        held.await.expect("the silent realm ends");
    }

    /// One HTTP answer on a fresh loopback listener; the URL to fetch it from.
    async fn serve_once(answer: &'static str) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a listener");
        let address = listener.local_addr().expect("an address");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("a connection");
            let mut head = vec![0; 4096];
            let _ = socket.read(&mut head).await;
            let _ = socket.write_all(answer.as_bytes()).await;
        });
        format!("http://{address}/certs")
    }

    const EMPTY_SET: &str =
        "HTTP/1.1 200 OK\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"keys\":[]}";

    #[tokio::test]
    async fn only_a_plain_http_url_is_fetched() {
        let verifier = Verifier::new("http://realm");
        for url in ["https://idm.city.invalid/certs", "ftp://idm/certs", "certs"] {
            let outcome = refresh(&verifier, url).await;
            assert!(
                matches!(outcome, Err(JwksError::Url(_))),
                "{url}: {outcome:?}"
            );
        }
        let outcome = refresh(&verifier, "http://exa mple/certs").await;
        assert!(matches!(outcome, Err(JwksError::Url(_))), "{outcome:?}");
    }

    #[tokio::test]
    async fn a_realm_answering_an_error_or_no_jwks_keeps_the_old_keys() {
        let verifier = Verifier::new("http://realm");
        for answer in [
            "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello",
            // a body cut short: the connection closes 95 bytes before its length
            "HTTP/1.1 200 OK\r\ncontent-length: 100\r\nconnection: close\r\n\r\n{\"keys\"",
        ] {
            let url = serve_once(answer).await;
            let outcome = refresh(&verifier, &url).await;
            assert!(
                matches!(&outcome, Err(JwksError::Unreachable(why)) if !why.is_empty()),
                "{answer}: {outcome:?}"
            );
        }
        let refused = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a listener");
        let closed = refused.local_addr().expect("an address");
        drop(refused);
        let outcome = refresh(&verifier, &format!("http://{closed}/certs")).await;
        assert!(
            matches!(outcome, Err(JwksError::Unreachable(_))),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn keeping_current_loads_the_keys_and_goes_on_refreshing() {
        let url = serve_once(EMPTY_SET).await;
        let verifier = Arc::new(Verifier::new("http://realm"));
        let task = tokio::spawn(keep_current_at(Arc::clone(&verifier), url, None));
        // The first refresh runs at once; the loop then sleeps until the next one.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!task.is_finished(), "the loop runs for ever");
        task.abort();
    }

    #[tokio::test]
    async fn a_failed_refresh_keeps_the_loop_alive() {
        // A bound socket that never listens: every connection to it is refused.
        let socket = tokio::net::TcpSocket::new_v4().expect("a socket");
        socket
            .bind("127.0.0.1:0".parse().expect("an address"))
            .expect("bound");
        let closed = socket.local_addr().expect("an address");
        let verifier = Arc::new(Verifier::new("http://realm"));
        let task = tokio::spawn(keep_current_at(
            Arc::clone(&verifier),
            "http://idm.city.invalid/certs".to_owned(),
            Some(closed),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !task.is_finished(),
            "a failed refresh does not end the loop"
        );
        task.abort();
        drop(socket);
    }
}
