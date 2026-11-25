//! The shared ingest listener of `streams` mode (ekiben extension, `--ingest-addr`): every
//! `http_server` input of every stream is served at `/{org}/{stream}{path}`.
//!
//! An unknown organization, stream or path answers 404; an endpoint answers as Bento's
//! `http_server` does, behind the input's `auth`.

use std::net::SocketAddr;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Router;

use crate::http_server::{
    lookup, mark_ingest_active, percent_decode, remote_addr, serve_request, text_response,
};

/// The ingest router; serve it with `into_make_service_with_connect_info::<SocketAddr>()`.
pub fn router() -> Router {
    Router::new().fallback(handle_ingest)
}

/// Serves the ingest listener on `listener` until the process ends; `http_server` inputs
/// connected from now on register here.
pub fn serve(
    listener: tokio::net::TcpListener,
) -> impl std::future::Future<Output = std::io::Result<()>> {
    mark_ingest_active();
    std::future::IntoFuture::into_future(axum::serve(
        listener,
        router().into_make_service_with_connect_info::<SocketAddr>(),
    ))
}

/// Marks the ingest listener as present without serving it (for a caller driving [`router`]
/// itself).
pub fn activate() {
    mark_ingest_active();
}

/// Splits `/{org}/{stream}{rest}`; `rest` is `/` when absent.
pub fn split_ingest_path(path: &str) -> Option<(String, String, String)> {
    let trimmed = path.strip_prefix('/')?;
    let mut it = trimmed.splitn(3, '/');
    let org = it.next().filter(|s| !s.is_empty())?;
    let stream = it.next().filter(|s| !s.is_empty())?;
    let rest = match it.next() {
        Some(r) => format!("/{r}"),
        None => "/".to_string(),
    };
    Some((org.to_string(), stream.to_string(), rest))
}

/// The `(org, stream, rest)` an ingest request path names, percent-decoded as [`handle`] reads it.
pub fn route_of(raw_path: &str) -> Option<(String, String, String)> {
    split_ingest_path(&percent_decode(raw_path))
}

/// Serves one ingest request as the listener does, for a router that serves other routes on
/// the same listener first (the `ngsi_ld` Notification routes).
pub async fn handle(req: Request) -> Response {
    handle_ingest(req).await
}

async fn handle_ingest(req: Request) -> Response {
    let remote = remote_addr(&req);
    let decoded = percent_decode(req.uri().path());
    let Some((org, stream, rest)) = split_ingest_path(&decoded) else {
        return text_response(StatusCode::NOT_FOUND, "404 page not found");
    };
    match lookup(&org, &stream, &rest) {
        Some((ep, vars)) => serve_request(ep, req, rest, vars, remote).await,
        None => text_response(StatusCode::NOT_FOUND, "404 page not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ingest_path_names_org_stream_and_rest() {
        assert_eq!(
            route_of("/org-a/air%20quality/sensors/1"),
            Some(("org-a".into(), "air quality".into(), "/sensors/1".into()))
        );
        assert_eq!(
            split_ingest_path("/org-a/s"),
            Some(("org-a".into(), "s".into(), "/".into()))
        );
        for bad in ["", "/", "/org-a", "/org-a/", "//s", "org-a/s"] {
            assert_eq!(split_ingest_path(bad), None, "{bad:?}");
        }
    }

    #[tokio::test]
    async fn the_listener_answers_a_path_no_stream_serves_with_a_refusal() {
        activate();
        assert!(crate::http_server::is_ingest_active());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve(listener));
        let resp = reqwest::get(format!("http://{addr}/no-org/no-stream/x"))
            .await
            .expect("the listener answers");
        assert!(resp.status().is_client_error(), "{}", resp.status());
        let direct = handle(
            axum::http::Request::builder()
                .uri("/no-org")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert!(direct.status().is_client_error(), "{}", direct.status());
    }
}
