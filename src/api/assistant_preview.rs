//! The embed dialog's live preview of a public assistant's launcher (T-4138, T-5328).
//!
//! A `srcdoc` frame inherits the Portal's own policy, whose `script-src 'self'` refuses the
//! launcher from `assistant.{domain}`, so the preview stayed blank with a CSP error in the
//! console. This page carries its own policy instead, outside the Portal's: a sandbox without an
//! origin, scripts and frames from the assistant host alone, nothing else fetched, and framed by
//! nobody but the Portal. It names no project and holds no data: the launcher line a site pastes,
//! for a public id the path gives.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::branding::Branding;
use crate::resource::is_dns1123;
use crate::state::AppState;

#[derive(Deserialize)]
struct PreviewQuery {
    lang: Option<String>,
}

/// The platform host, when every label of it is a DNS label: it goes into a header and a page.
fn host(domain: &str) -> Option<&str> {
    (!domain.is_empty() && domain.split('.').all(is_dns1123)).then_some(domain)
}

/// A language the launcher reads (`data-lang`): two or three lowercase letters.
fn language(lang: Option<&str>) -> Option<&str> {
    lang.filter(|lang| {
        (2..=3).contains(&lang.len()) && lang.bytes().all(|b| b.is_ascii_lowercase())
    })
}

/// The preview's policy: scripts, images and the chat frame from the assistant host alone.
fn policy(host: &str) -> String {
    format!(
        "sandbox allow-scripts; default-src 'none'; script-src https://assistant.{host}; \
         style-src 'unsafe-inline'; img-src https://assistant.{host} data:; \
         frame-src https://assistant.{host}; base-uri 'none'; form-action 'none'; \
         frame-ancestors 'self'"
    )
}

/// A blank page holding the launcher alone, as the dialog's snippet prints it.
fn document(host: &str, public_id: &str, lang: Option<&str>) -> String {
    let data_lang = lang
        .map(|lang| format!(" data-lang=\"{lang}\""))
        .unwrap_or_default();
    format!(
        "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\"></head><body>\
         <script src=\"https://assistant.{host}/d/{public_id}/embed.js\"{data_lang} defer></script>\
         </body></html>",
        lang.unwrap_or("sk")
    )
}

/// `/assistant-preview/{publicId}`: 404 for an id that is no DNS label or a Portal without a domain.
async fn preview(
    State(state): State<AppState>,
    Path(public_id): Path<String>,
    Query(query): Query<PreviewQuery>,
) -> Response {
    let branding = Branding::load(state.config.branding_file.as_deref());
    let (Some(host), true) = (host(&branding.domain), is_dns1123(&public_id)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let lang = language(query.lang.as_deref());
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::CONTENT_SECURITY_POLICY, policy(host)),
            (header::X_FRAME_OPTIONS, "SAMEORIGIN".to_owned()),
        ],
        document(host, &public_id, lang),
    )
        .into_response()
}

pub fn router() -> Router<AppState> {
    Router::new().route("/assistant-preview/{public_id}", get(preview))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_allows_the_assistant_host_alone_without_an_origin() {
        let csp = policy("dev.example.com");
        assert!(csp.starts_with("sandbox allow-scripts;"));
        assert!(!csp.contains("allow-same-origin"));
        assert!(csp.contains("script-src https://assistant.dev.example.com;"));
        assert!(csp.contains("default-src 'none'"));
        assert!(csp.contains("frame-ancestors 'self'"));
    }

    #[test]
    fn a_host_or_language_that_could_break_out_is_refused() {
        assert_eq!(host("dev.example.com"), Some("dev.example.com"));
        assert_eq!(host(""), None);
        assert_eq!(host("evil.com; script-src *"), None);
        assert_eq!(host("a\"><script>"), None);
        assert_eq!(language(Some("en")), Some("en"));
        assert_eq!(language(Some("en\"x")), None);
        assert_eq!(language(Some("EN")), None);
        assert_eq!(language(None), None);
    }

    #[test]
    fn the_document_holds_the_launcher_line_the_dialog_prints() {
        let html = document("dev.example.com", "helsinki-catalogue", Some("en"));
        assert!(html.contains(
            "<script src=\"https://assistant.dev.example.com/d/helsinki-catalogue/embed.js\" data-lang=\"en\" defer></script>"
        ));
        assert!(html.contains("<html lang=\"en\">"));
        assert!(document("dev.example.com", "a", None).contains("<html lang=\"sk\">"));
    }
}
