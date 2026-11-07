//! The App templates' live demos at `/templates/{name}/` (T-3306, AP-141).
//!
//! One bundle, `sdk/dist/demos` (`vite.demos.config.ts`), renders each sample over the template
//! with the rows of its `fixtures.ts`: no session, no token, no data of the platform. It carries
//! its own Content Security Policy, outside the Portal's, which forbids every connection, so a
//! demo cannot reach any server even if a sample tried. The page is sandboxed without an origin
//! (T-3310): its script cannot read the Portal's cookies or storage, nor frame a Portal page.
//! Its scripts load from that opaque origin, so the assets answer any origin (they are the
//! repository's own code and carry no data), and the map's worker comes inline, the one way a
//! document without an origin may start one (`mapWorkerReady`, as the code preview does).

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use rust_embed::RustEmbed;

use crate::agents::samples;
use crate::state::AppState;

/// Built with the Portal's image from the SDK; empty in a plain `cargo test`, where every
/// demo answers 404.
#[derive(RustEmbed)]
#[folder = "sdk/dist/demos"]
struct Demos;

/// The demos' policy: a sandbox with scripts and no origin, their own scripts, styles and images,
/// the map's worker inline, nothing fetched from anywhere, no frame of their own, and framed by
/// nobody but the Portal.
pub const DEMO_CSP: &str = "sandbox allow-scripts; default-src 'self'; base-uri 'self'; \
     object-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
     img-src 'self' data: blob:; font-src 'self' data:; worker-src 'self' blob: data:; \
     connect-src 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'self'";

/// The demo page with the map's worker inline before `</head>`, when the Portal was built with it.
fn with_worker(html: &str, worker: Option<String>) -> String {
    match worker {
        Some(worker) => html.replacen(
            "</head>",
            &format!("<script id=\"kit-worker\" type=\"text/plain\">{worker}</script>\n</head>"),
            1,
        ),
        None => html.to_owned(),
    }
}

fn file(path: &str) -> Response {
    let Some(content) = Demos::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let body = if path == "index.html" {
        with_worker(
            &String::from_utf8_lossy(&content.data),
            crate::agents::kit::worker(),
        )
        .into_bytes()
    } else {
        content.data.into_owned()
    };
    respond(path, body)
}

/// One file with its type, the demos' policy and, for an asset, any origin: the sandboxed page
/// asks for its modules from an origin of none.
fn respond(path: &str, body: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Body::from(body).into_response();
    let headers = response.headers_mut();
    if path.starts_with("assets/") {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
    }
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(DEMO_CSP),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("SAMEORIGIN"));
    response
}

/// `/templates/{name}/`: the demo page, for a name the gallery holds and nothing else.
async fn demo(Path(name): Path<String>) -> Response {
    if !samples::templates()
        .iter()
        .any(|template| template.name == name)
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    file("index.html")
}

/// `/templates/assets/{file}`: the bundle's scripts, styles and images.
async fn asset(Path(name): Path<String>) -> Response {
    if name.contains('/') || name.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    file(&format!("assets/{name}"))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/templates/assets/{file}", get(asset))
        .route("/templates/{name}", get(demo))
        .route("/templates/{name}/", get(demo))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_forbids_every_connection_and_foreign_framing() {
        assert!(DEMO_CSP.contains("connect-src 'none'"));
        assert!(DEMO_CSP.contains("frame-ancestors 'self'"));
        assert!(!DEMO_CSP.contains("unsafe-eval"));
    }

    // T-3310: the demo runs without an origin, so its script reads no cookie or storage of the
    // Portal's and frames no Portal page.
    #[test]
    fn the_demo_is_sandboxed_without_an_origin_and_frames_nothing() {
        assert!(DEMO_CSP.starts_with("sandbox allow-scripts;"));
        assert!(!DEMO_CSP.contains("allow-same-origin"));
        assert!(DEMO_CSP.contains("frame-src 'none'"));
        assert!(DEMO_CSP.contains("worker-src 'self' blob: data:"));
    }

    #[test]
    fn an_asset_answers_any_origin_and_the_page_does_not() {
        let asset = respond("assets/main.js", b"export {}".to_vec());
        assert_eq!(asset.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(asset.headers()[header::CONTENT_SECURITY_POLICY], DEMO_CSP);
        let page = respond("index.html", b"<html></html>".to_vec());
        assert!(page
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());
        assert_eq!(page.headers()[header::CONTENT_SECURITY_POLICY], DEMO_CSP);
    }

    #[test]
    fn the_page_carries_the_map_worker_inline_when_there_is_one() {
        let html = "<html><head><title>x</title></head><body></body></html>";
        assert_eq!(
            with_worker(html, Some("AAAA".into())),
            "<html><head><title>x</title><script id=\"kit-worker\" type=\"text/plain\">AAAA</script>\n</head><body></body></html>"
        );
        assert_eq!(with_worker(html, None), html);
    }
}
