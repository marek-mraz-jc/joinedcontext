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
pub async fn serve(listener: tokio::net::TcpListener) -> std::io::Result<()> {
    mark_ingest_active();
    axum::serve(
        listener,
        router().into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
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
