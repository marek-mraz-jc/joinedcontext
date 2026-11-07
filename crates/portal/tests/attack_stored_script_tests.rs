//! Attack vector T-1687 (AG-46, PF-50): stored and reflected script in anything a person types.
//!
//! The Portal's own pages are React, which renders every string as text, and the Portal's CSP has
//! no `unsafe-inline` for scripts. This file is the server's half: the API answers that carry the
//! strings a person typed back, and the names refused before they are stored. The kit preview
//! document the Portal used to build out of typed strings is gone with the kit (T-0681); a code
//! preview runs the App's own React on the SDK runtime.
//!
//! What the existing suites already play, so this file does not repeat it:
//! `ui/tests/browser_security.test.tsx::a_javascript_url_from_a_manifest_is_text` (a
//! `javascript:` URL in a manifest is rendered without a link, and no page hands a raw value to
//! an anchor), `::a_preview_frame_cannot_reach_the_portal_origin`,
//! `security_headers_tests::the_portal_sends_a_csp_without_unsafe_inline_scripts` and
//! `::every_answer_carries_the_headers_that_do_not_depend_on_the_route` (the policy that would
//! stop an inline script even if one got through).

mod common;

use axum::http::{header, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::auth::session::Identity;

/// Everything a person can type that ends up inside an HTML document, in one string.
const HOSTILE: &str = "</script><script>alert('jc')</script><img src=x onerror=alert(1)>";

fn admin() -> Identity {
    let mut identity = common::person("admin");
    identity.groups = vec!["portal-approver".to_owned()];
    identity
}

/// PF-50: a name is a DNS-1123 label, so a name shaped like markup is refused before it is
/// stored anywhere — the refusal is the shape of the name and says what a name may be.
#[tokio::test]
async fn a_name_shaped_like_markup_is_refused_before_it_is_stored() {
    let state = common::state_on_forge().await;
    let answer = common::send(
        &state,
        admin(),
        "POST",
        "/api/v1/projects/ovzdusie/spaces?dryRun=All",
        Some(json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "ContextSpace",
            "metadata": { "name": HOSTILE, "namespace": "ovzdusie" },
            "spec": { "isSandbox": true },
        })),
    )
    .await;
    assert_ne!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    assert!(
        answer.text.to_lowercase().contains("dns")
            || answer.text.contains("name")
            || answer.text.contains("label"),
        "the refusal has to say what a name may be: {}",
        answer.text
    );
}

/// AG-46: a title and a description are free text, so markup in them is kept — and every answer
/// that carries it is `application/json` with the Portal's policy on it, so nothing renders it as
/// markup and an inline script would not run even if something did.
#[tokio::test]
async fn markup_a_person_typed_travels_as_json_under_the_policy() {
    let state = common::state_on_forge().await;
    let answer = common::send(
        &state,
        admin(),
        "POST",
        "/api/v1/projects/ovzdusie/spaces?dryRun=All",
        Some(json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "ContextSpace",
            "metadata": {
                "name": "mobility",
                "namespace": "ovzdusie",
                "title": HOSTILE,
                "description": HOSTILE,
            },
            "spec": { "isSandbox": true },
        })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(
        body["valid"],
        json!(true),
        "free text is kept, not refused: {}",
        answer.text
    );
    // The same request through the router again, so the headers of the answer can be read: the
    // policy and the content type are what keep typed markup from ever being a document.
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces?dryRun=All")
                .header(header::COOKIE, common::cookie(&state.config, admin()))
                .header(joinedcontext_portal::auth::csrf::CSRF_HEADER, common::CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    json!({
                        "apiVersion": "joinedcontext.com/v1alpha1",
                        "kind": "ContextSpace",
                        "metadata": {
                            "name": "mobility",
                            "namespace": "ovzdusie",
                            "title": HOSTILE,
                        },
                        "spec": { "isSandbox": true },
                    })
                    .to_string(),
                ))
                .expect("a request"),
        )
        .await
        .expect("a response");
    let header_of = |name: header::HeaderName| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    assert!(
        header_of(header::CONTENT_TYPE).starts_with("application/json"),
        "an answer carrying typed markup must not be served as a document: {}",
        header_of(header::CONTENT_TYPE)
    );
    let policy = header_of(header::CONTENT_SECURITY_POLICY);
    assert!(
        policy.contains("default-src 'self'") && !policy.contains("script-src"),
        "scripts fall back to default-src 'self', which has no 'unsafe-inline': {policy}"
    );
}
