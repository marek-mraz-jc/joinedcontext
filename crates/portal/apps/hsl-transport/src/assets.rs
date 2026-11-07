//! The React build, embedded in the binary so the image is one artifact (AP-25).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

use crate::{App, Config};

#[derive(RustEmbed)]
#[folder = "ui/dist"]
pub struct Assets;

/// Shown when the binary was built without a UI bundle, so the reason is on the page instead
/// of in a log nobody reads.
pub const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8" /><title>HSL live</title></head>
<body><p>UI bundle not built. Run <code>pnpm build</code> in <code>ui/</code>.</p></body>
</html>
"#;

pub async fn static_handler(State(app): State<Arc<App>>, uri: Uri) -> Response {
    let base = app.config.base_path.trim_end_matches('/');
    let path = uri
        .path()
        .strip_prefix(base)
        .unwrap_or(uri.path())
        .trim_start_matches('/');
    if path.is_empty() || path == "index.html" {
        return index(&app.config);
    }
    let Some(file) = Assets::get(path) else {
        // Anything with an extension is a real miss; everything else is a client route.
        return if path.rsplit('/').next().unwrap_or(path).contains('.') {
            StatusCode::NOT_FOUND.into_response()
        } else {
            index(&app.config)
        };
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref()),
            (header::CACHE_CONTROL, cache),
        ],
        Body::from(file.data),
    )
        .into_response()
}

/// The front page, with the reconciler's `#jc-config` first in its head when it was handed one
/// (AP-67, AP-126).
fn index(config: &Config) -> Response {
    let html = Assets::get("index.html")
        .map(|file| String::from_utf8_lossy(&file.data).into_owned())
        .unwrap_or_else(|| PLACEHOLDER_HTML.to_owned());
    let body = match &config.page_config {
        Some(json) => with_config(&html, json),
        None => html,
    };
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        Body::from(body),
    )
        .into_response()
}

/// `html` with `<script id="jc-config">` holding `json` right after `<head>`, where the page's
/// scripts find it before they run.
fn with_config(html: &str, json: &str) -> String {
    let element = format!(r#"<script id="jc-config" type="application/json">{json}</script>"#);
    let at = html
        .to_ascii_lowercase()
        .find("<head>")
        .map_or(0, |at| at + "<head>".len());
    format!("{}{element}{}", &html[..at], &html[at..])
}
