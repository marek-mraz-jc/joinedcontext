//! The React build, embedded in the binary so the image is one artifact (AP-25).

use std::borrow::Cow;
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
<head><meta charset="utf-8" /><title>Air quality</title></head>
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
    file_response(path, file.data)
}

/// One file of the bundle: its type from the name, and kept for a year when it is a hashed
/// `assets/` file, which never changes under the same name.
fn file_response(path: &str, data: Cow<'static, [u8]>) -> Response {
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
        Body::from(data),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hashed_asset_is_kept_for_a_year_and_anything_else_is_asked_for_again() {
        let script = file_response("assets/index-1a2b.js", Cow::Borrowed(b"export {}"));
        assert_eq!(
            script.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert!(script.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("javascript"));
        let icon = file_response("favicon.svg", Cow::Borrowed(b"<svg/>"));
        assert_eq!(icon.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(icon.headers()[header::CONTENT_TYPE], "image/svg+xml");
        let unknown = file_response("data.unknownext", Cow::Borrowed(b""));
        assert_eq!(
            unknown.headers()[header::CONTENT_TYPE],
            "application/octet-stream"
        );
    }
}
