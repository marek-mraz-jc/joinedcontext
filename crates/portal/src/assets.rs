use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, StatusCode, Uri};
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
/// from the build embedded in the binary, with SPA routing either way. Only GET and HEAD: another
/// method on a path no route serves answers 405, so a misrouted write, such as a POST to an
/// internal hook on the public base, never looks like a success (F8).
pub async fn static_handler(State(state): State<AppState>, method: Method, uri: Uri) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET, HEAD")],
        )
            .into_response();
    }
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
        return static_handler(State(state), Method::GET, uri).await;
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
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static(cache_control(path)),
            ),
        ],
        Body::from_stream(answer.bytes_stream()),
    )
        .into_response()
}

fn embedded(path: &str) -> Response {
    served(path, Assets::get(path))
}

/// The answer of the embedded file at `path`, a 404 when the build holds none: apart from
/// [`embedded`], so a test hands it a file when the UI is not built.
fn served(path: &str, file: Option<rust_embed::EmbeddedFile>) -> Response {
    let Some(file) = file else {
        return ApiError::NotFound(format!("Asset '{path}' not found")).into_response();
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.as_ref()),
            (header::CACHE_CONTROL, cache_control(path)),
        ],
        Body::from(file.data),
    )
        .into_response()
}

fn serve_index() -> Response {
    index_page(Assets::get("index.html"))
}

/// The shell of the SPA: the built `index.html`, or [`PLACEHOLDER_HTML`] before a UI build.
fn index_page(file: Option<rust_embed::EmbeddedFile>) -> Response {
    let page = match file {
        Some(file) => file.data,
        None => std::borrow::Cow::Borrowed(PLACEHOLDER_HTML.as_bytes()),
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        Body::from(page),
    )
        .into_response()
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

    /// A file as the embed hands it, read from a scratch copy: the test build embeds no UI.
    fn file(name: &str, body: &[u8]) -> rust_embed::EmbeddedFile {
        let path = std::env::temp_dir().join(format!("t7858-assets-{}-{name}", std::process::id()));
        std::fs::write(&path, body).expect("the scratch file");
        let file = rust_embed::utils::read_file_from_fs(&path).expect("read back");
        let _ = std::fs::remove_file(&path);
        file
    }

    async fn text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body");
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn an_embedded_file_is_served_with_its_type_and_its_cache_rule() {
        let response = served("assets/app-1a2b.js", Some(file("app.js", b"export {}")));
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers()[header::CONTENT_TYPE]
            .to_str()
            .expect("text")
            .contains("javascript"));
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(text(response).await, "export {}");
        let missing = served("assets/gone.js", None);
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_shell_is_the_built_index_or_the_placeholder_and_never_cached() {
        let built = index_page(Some(file("index.html", b"<!doctype html><p>built</p>")));
        assert_eq!(built.status(), StatusCode::OK);
        assert_eq!(built.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(
            built.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(text(built).await, "<!doctype html><p>built</p>");
        let placeholder = index_page(None);
        assert_eq!(placeholder.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(text(placeholder).await, PLACEHOLDER_HTML);
    }

    /// The derive's lookups that need no built UI: a path outside the folder, a missing file,
    /// and the trait's own entry points.
    #[test]
    fn the_embed_answers_nothing_outside_its_folder_or_missing() {
        assert!(Assets::get("../Cargo.toml").is_none());
        assert!(Assets::get("no-such-file.js").is_none());
        assert!(<Assets as RustEmbed>::get("no-such-file.js").is_none());
        let listed: Vec<_> = <Assets as RustEmbed>::iter().collect();
        assert_eq!(listed.len(), Assets::iter().count());
    }
}
