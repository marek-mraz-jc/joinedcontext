use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

use crate::auth::session::CurrentUser;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(RustEmbed)]
#[folder = "ui/dist"]
pub struct Assets;

/// Fallback HTML served when the UI bundle has not been built yet.
pub const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>joinedcontext Portal</title>
</head>
<body>
    <div id="root">
        <h1>joinedcontext Portal</h1>
        <p>UI bundle not built. Run <code>pnpm build</code> in <code>ui/</code> to produce production assets.</p>
    </div>
</body>
</html>
"#;

/// A fingerprinted file under `assets/` never changes; everything else is asked again.
fn cache_control(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

/// A path without an extension in its last segment is a page of the SPA: the shell answers it.
fn is_page(path: &str) -> bool {
    path.is_empty() || !path.rsplit('/').next().unwrap_or(path).contains('.')
}

/// Serves the UI: from the `portal-ui` bundle when `JC_PORTAL_UI_URL` names one (T-2485), else
/// from the build embedded in the binary, with SPA routing either way.
pub async fn static_handler(State(state): State<AppState>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if is_page(path) { "index.html" } else { path };
    match state.config.ui_url.as_deref() {
        Some(ui) => from_bundle(ui, path).await,
        None if path == "index.html" => serve_index(),
        None => embedded(path),
    }
}

/// The front door, `/` (T-5627, EP-81): a signed-in person gets the home page, a visitor without
/// a session the public catalogue, so a citizen who types the installation's name sees its data
/// rather than a login. The edge lets `/` through without a session for this (portal-home).
pub async fn home(
    user: Result<CurrentUser, ApiError>,
    State(state): State<AppState>,
    uri: Uri,
) -> Response {
    if user.is_ok() {
        return static_handler(State(state), uri).await;
    }
    let target = match uri.query() {
        Some(query) => format!("/catalogue?{query}"),
        None => "/catalogue".to_owned(),
    };
    // The answer depends on the session: no cache may hand it to the next person.
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, target.as_str()),
            (header::CACHE_CONTROL, "no-store"),
        ],
    )
        .into_response()
}

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default()
    })
}

/// One file of the bundle, read from `portal-ui` inside the cluster. The Portal's own layers add
/// the security headers, so the bundle's server only has to hand over bytes and their type.
async fn from_bundle(ui: &str, path: &str) -> Response {
    // nginx keeps a `..` inside its root, but a path that climbs is nobody's asset.
    if path.split('/').any(|segment| segment == "..") {
        return ApiError::NotFound(format!("Asset '{path}' not found")).into_response();
    }
    let answer = match http().get(format!("{ui}/{path}")).send().await {
        Ok(answer) => answer,
        Err(err) => {
            tracing::warn!(error = %err, "the UI bundle did not answer");
            return ApiError::Unavailable("the UI bundle is not reachable".into()).into_response();
        }
    };
    match answer.status() {
        reqwest::StatusCode::OK => {}
        reqwest::StatusCode::NOT_FOUND => {
            return ApiError::NotFound(format!("Asset '{path}' not found")).into_response();
        }
        status => {
            tracing::warn!(%status, "the UI bundle refused a file");
            return ApiError::Unavailable("the UI bundle is not reachable".into()).into_response();
        }
    }
    let mime = answer
        .headers()
        .get(header::CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| {
            HeaderValue::from_str(mime_guess::from_path(path).first_or_octet_stream().as_ref())
                .unwrap_or(HeaderValue::from_static("application/octet-stream"))
        });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, cache_control(path))
        .body(Body::from_stream(answer.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn embedded(path: &str) -> Response {
    let Some(file) = Assets::get(path) else {
        return ApiError::NotFound(format!("Asset '{path}' not found")).into_response();
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, cache_control(path))
        .body(Body::from(file.data))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn serve_index() -> Response {
    if let Some(file) = Assets::get("index.html") {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(file.data))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    } else {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(PLACEHOLDER_HTML))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_html_is_valid() {
        assert!(PLACEHOLDER_HTML.contains("<!doctype html>"));
        assert!(PLACEHOLDER_HTML.contains("UI bundle not built"));
        assert!(PLACEHOLDER_HTML.contains("</html>"));
    }

    #[test]
    fn mime_of_app_js_is_javascript() {
        let mime = mime_guess::from_path("app.js").first_or_octet_stream();
        assert!(
            mime.as_ref().contains("javascript"),
            "expected mime to contain 'javascript', got {mime}"
        );
    }
}
