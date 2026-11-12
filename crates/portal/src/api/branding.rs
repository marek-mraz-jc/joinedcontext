//! `GET /api/v1/branding`: what this installation is called and what it looks like (UI-30).
//!
//! Public and cacheable on purpose. The block holds no secret, and the login page needs the
//! instance name and the logo before anyone has signed in.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;

use crate::branding::Branding;
use crate::error::ApiError;
use crate::state::AppState;

/// How long a browser may keep the branding before asking again. Long enough that a reload
/// costs nothing, short enough that a rebrand shows up without anybody clearing a cache.
pub const MAX_AGE_SECONDS: u32 = 300;

#[utoipa::path(
    get,
    path = "/api/v1/branding",
    summary = "Read Branding",
    description = "The installation's name, colours and logo as the Portal and the catalogue theme draw them. Public: it names nothing of any project.",
    tag = "system",
    responses(
        (status = 200, description = "The branding of this installation", body = Branding)
    )
)]
pub async fn get_branding(State(state): State<AppState>) -> impl IntoResponse {
    let mut branding = state.branding();
    branding.apps_origin = state
        .config
        .apps_url
        .as_ref()
        .map(|url| url.origin().ascii_serialization());
    branding.hidden_sections = hidden_sections(&state.config);
    (
        [(
            header::CACHE_CONTROL,
            format!("public, max-age={MAX_AGE_SECONDS}"),
        )],
        Json(branding),
    )
}

/// The project sections the UI leaves out of its navigation and routes (T-2874).
pub fn hidden_sections(config: &crate::config::Config) -> Vec<String> {
    if config.dashboards {
        Vec::new()
    } else {
        vec!["dashboards".to_owned()]
    }
}

/// The logo or the favicon, read from beside the branding file (UI-30).
///
/// The file name comes from the branding block, which already refused anything but a
/// same-origin file name, and it is joined to the branding file's own directory: the two
/// assets a ConfigMap carries are the only files this route can ever reach. Served as an
/// image, which is the only way the UI uses it, so an SVG cannot run script.
#[utoipa::path(
    get,
    path = "/api/v1/branding/{asset}",
    tag = "system",
    params(("asset" = String, Path, description = "`logo` or `favicon`")),
    responses(
        (status = 200, description = "The image", content_type = "image/svg+xml"),
        (status = 404, description = "No such asset is configured", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_asset(
    State(state): State<AppState>,
    Path(asset): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let branding = state.branding();
    let name = match asset.as_str() {
        "logo" => branding.logo,
        "favicon" => branding.favicon,
        other => return Err(ApiError::NotFound(format!("no branding asset '{other}'"))),
    };
    if name.is_empty() {
        return Err(ApiError::NotFound(format!("no {asset} is configured")));
    }
    // The request's Organization carries its own as an SVG in its manifest (UIX-16), checked
    // when the Change was proposed; served as an attachment that never sniffs into a page, so
    // even an SVG the check missed renders only inside an `<img>` (RTE-14).
    if name == Branding::INLINE_SVG {
        let svg = state
            .pinned_organization()
            .and_then(|spec| match asset.as_str() {
                "logo" => spec.appearance.logo_svg,
                _ => spec.appearance.favicon_svg,
            });
        let svg = svg.ok_or_else(|| ApiError::NotFound(format!("no {asset} is configured")))?;
        return Ok(served(&format!("{asset}.svg"), svg.into_bytes()));
    }
    let directory = state
        .config
        .branding_file
        .as_deref()
        .map(std::path::Path::new)
        .and_then(std::path::Path::parent)
        .ok_or_else(|| ApiError::NotFound(format!("no {asset} is configured")))?;

    let bytes = std::fs::read(directory.join(&name))
        .map_err(|_| ApiError::NotFound(format!("the {asset} file is not readable")))?;
    Ok(served(&name, bytes))
}

/// An asset's answer: its image type and `Content-Disposition: attachment` (the server adds
/// `nosniff`), so opening the address downloads it rather than showing
/// it as a document; an `<img>` still draws it (RTE-14).
fn served(name: &str, bytes: Vec<u8>) -> axum::response::Response {
    let file = name.rsplit('/').next().unwrap_or("asset");
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, media_type(name).to_owned()),
            (
                header::CACHE_CONTROL,
                format!("public, max-age={MAX_AGE_SECONDS}"),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file}\""),
            ),
        ],
        bytes,
    )
        .into_response()
}

/// The image types a logo or a favicon may be. Anything else is served as a download rather
/// than as a document, so an HTML file dropped into the ConfigMap cannot become a page.
fn media_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or_default() {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/vnd.microsoft.icon",
        _ => "application/octet-stream",
    }
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/branding", axum::routing::get(get_branding))
        .route("/branding/{asset}", axum::routing::get(get_asset))
}
