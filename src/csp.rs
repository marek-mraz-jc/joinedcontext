//! The Portal's own Content-Security-Policy with a fresh style nonce on every answer (T-5828).
//!
//! `style-src` names no `'unsafe-inline'`: a `<style>` element applies only when it carries the
//! answer's nonce. The SPA shell is built with [`NONCE_SLOT`] as its nonce (vite `html.cspNonce`)
//! and this layer writes the answer's nonce into it, so the shell's own styles and the
//! `<meta property="csp-nonce">` the UI stamps its runtime stylesheets with (ui/src/cspNonce.ts)
//! carry the value the header names. Markup injected into a page gets no nonce, so it cannot
//! bring a stylesheet (CSS selectors that read attribute values out of the page).

use argon2::password_hash::rand_core::{OsRng, RngCore};
use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use std::sync::Arc;

/// What the policy and the shell carry where the answer's nonce goes.
pub const NONCE_SLOT: &str = "__JC_CSP_NONCE__";

/// The largest HTML answer the layer reads to write the nonce in; the shell is a few KB and a
/// public docs page well under this.
const MAX_HTML: usize = 4 << 20;

/// A nonce of 128 random bits, base64 (CSP3 asks for at least 128).
fn nonce() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Sets `policy` (a policy with [`NONCE_SLOT`] where the nonce goes) on the answer with a fresh
/// nonce, and writes the same nonce into an HTML answer's slots.
pub async fn middleware(State(policy): State<Arc<str>>, request: Request, next: Next) -> Response {
    let nonce = nonce();
    let response = next.run(request).await;
    let Ok(value) = HeaderValue::from_str(&policy.replace(NONCE_SLOT, &nonce)) else {
        tracing::error!("the Content-Security-Policy is not a header value");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    let mut response = if html {
        let (mut parts, body) = response.into_parts();
        let bytes = match to_bytes(body, MAX_HTML).await {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(error = %err, "an HTML answer could not be read to stamp its nonce");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        let text = String::from_utf8_lossy(&bytes).replace(NONCE_SLOT, &nonce);
        parts.headers.remove(header::CONTENT_LENGTH);
        Response::from_parts(parts, Body::from(text))
    } else {
        response
    };
    response
        .headers_mut()
        .insert(header::CONTENT_SECURITY_POLICY, value);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    const POLICY: &str = "default-src 'self'; style-src 'self' 'nonce-__JC_CSP_NONCE__'";

    fn app() -> Router {
        Router::new()
            .route(
                "/page",
                get(|| async {
                    axum::response::Html(r#"<style nonce="__JC_CSP_NONCE__">a{}</style>"#)
                }),
            )
            .route("/json", get(|| async { "__JC_CSP_NONCE__" }))
            .layer(from_fn_with_state(Arc::from(POLICY), middleware))
    }

    async fn get_page(uri: &str) -> (String, String) {
        let response = app()
            .oneshot(Request::get(uri).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .expect("ascii")
            .to_owned();
        let body = to_bytes(response.into_body(), 1 << 20).await.expect("body");
        (policy, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn the_page_and_the_policy_carry_one_fresh_nonce() {
        let (policy, body) = get_page("/page").await;
        let nonce = policy
            .split("'nonce-")
            .nth(1)
            .and_then(|rest| rest.split('\'').next())
            .expect("a nonce in the policy");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(nonce)
                .expect("base64")
                .len(),
            16
        );
        assert_eq!(body, format!(r#"<style nonce="{nonce}">a{{}}</style>"#));
        let (again, _) = get_page("/page").await;
        assert_ne!(policy, again, "every answer gets its own nonce");
    }

    #[tokio::test]
    async fn an_answer_other_than_html_keeps_its_body() {
        let (policy, body) = get_page("/json").await;
        assert!(!policy.contains(NONCE_SLOT), "{policy}");
        assert_eq!(body, NONCE_SLOT);
    }
}
