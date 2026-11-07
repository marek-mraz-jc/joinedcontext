//! Keeps a cookie session alive for as long as the Keycloak SSO session allows (CC-40, CC-42).
//!
//! The access token Keycloak issues lives five minutes; the session must not. Inside the leeway
//! before it expires, the first request that notices trades the refresh token for a new one,
//! rotates both cookies and carries on. Only a refused refresh ends the session: the cookies are
//! cleared and a browser navigation is sent to the login page, an API call gets `401`.

use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::PrivateCookieJar;

use crate::auth::session::{self, Front};
use crate::error::ApiError;
use crate::state::AppState;

/// Paths the refresh must leave alone: the login flow mints the session, logout ends it, the
/// back channel carries no session, and static assets carry nothing worth a token round trip.
fn is_exempt(path: &str) -> bool {
    path.starts_with("/assets/")
        || path.starts_with("/api/v1/auth/") && path != "/api/v1/auth/me"
        || path == "/login"
}

/// A top-level browser navigation, as opposed to a `fetch` from the SPA.
fn is_navigation(request: &Request) -> bool {
    accepts_html(request.headers())
}

/// Whether the caller asked for a page (`Accept: text/html`): a browser navigating, which a
/// refusal sends to the login page, rather than a script, which gets a 401.
pub(crate) fn accepts_html(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

/// The login page with the interrupted location to come back to.
pub fn login_redirect(path_and_query: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("redirect_to", path_and_query)
        .finish();
    format!("/login?{query}")
}

/// Middleware over the portal routes: refreshes a session that is due, ends one that cannot be.
pub async fn middleware(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    // A bearer caller and the edge's user token are verified per request and never refreshed
    // by the portal: the edge refreshes its own session (ADR-N-019).
    if is_exempt(&path)
        || Front::of(request.headers(), state.config.trust_edge_token) != Front::Portal
    {
        return next.run(request).await;
    }
    let jar = PrivateCookieJar::from_headers(request.headers(), state.config.cookie_key.clone());
    // A session sealed with a key a rotation is still letting in is read here too, and the
    // refresh below re-seals it with the active one (T-0973).
    let Some(current) = session::load_with(request.headers(), &state) else {
        return next.run(request).await;
    };
    let now = session::now_unix();
    if !current.needs_refresh(now) {
        return next.run(request).await;
    }
    let refreshed = match state.oidc.as_deref() {
        Some(client) => client.refresh(&current).await,
        None => Err(ApiError::Unavailable(
            "no identity provider is configured".into(),
        )),
    };
    match refreshed {
        Ok(fresh) => {
            let mut request = request;
            request.extensions_mut().insert(fresh.clone());
            let response = next.run(request).await;
            match session::store(jar, &fresh) {
                Ok(jar) => {
                    state.keep_session_claims(&fresh);
                    (jar, response).into_response()
                }
                Err(err) => err.into_response(),
            }
        }
        // Still inside the leeway: this request runs on the token it has, the next one retries.
        // Parallel requests from one page share a refresh token this way without a lock.
        Err(_) if !current.access_expired(now) => next.run(request).await,
        Err(_) => {
            tracing::info!(subject = %current.identity.subject, "session ended: refresh refused");
            let jar = session::clear(jar);
            if is_navigation(&request) {
                let target = request
                    .uri()
                    .path_and_query()
                    .map(|pq| pq.as_str().to_string())
                    .unwrap_or_else(|| path.clone());
                (jar, Redirect::to(&login_redirect(&target))).into_response()
            } else {
                (jar, ApiError::Unauthorized).into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------------------
    // T-2099: the edge cases of the middleware's two decisions — whose request to leave alone,
    // and what a refusal does to the browser.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn the_exemption_is_by_whole_path_and_cannot_be_widened_by_a_prefix() {
        // A path that merely *starts* like an exempt one is not exempt: a route added under
        // `/api/v1/authorisation/` or a file at `/assets-of-mine` must still refresh its session.
        for guarded in [
            "/api/v1/auth/me",
            "/api/v1/authorisation/roles",
            "/api/v1/authx/login",
            "/assets-of-mine/app.js",
            "/login-as/someone",
            "/projects/helsinki/spaces",
            "/api/v1/projects/helsinki/changes",
            "/",
        ] {
            assert!(!is_exempt(guarded), "{guarded} was treated as exempt");
        }
        for exempt in [
            "/api/v1/auth/login",
            "/api/v1/auth/callback",
            "/api/v1/auth/logout",
            "/api/v1/auth/backchannel-logout",
            "/assets/index-abc123.js",
            "/assets/",
            "/login",
        ] {
            assert!(is_exempt(exempt), "{exempt} lost its exemption");
        }
    }

    #[test]
    fn only_a_page_navigation_is_sent_to_the_login_and_a_fetch_is_not() {
        // A redirect answered to `fetch` would land the login page's HTML inside a JSON parse; the
        // SPA has to get 401 and decide for itself (CC-42).
        for (accept, navigation) in [
            ("text/html,application/xhtml+xml", true),
            ("text/html", true),
            ("application/json", false),
            ("application/problem+json", false),
            ("*/*", false),
            ("", false),
        ] {
            let mut builder = Request::builder();
            if !accept.is_empty() {
                builder = builder.header(header::ACCEPT, accept);
            }
            let request = builder.body(axum::body::Body::empty()).unwrap();
            assert_eq!(is_navigation(&request), navigation, "{accept:?}");
        }
        // No Accept at all is not a navigation either.
        let bare = Request::builder().body(axum::body::Body::empty()).unwrap();
        assert!(!is_navigation(&bare));
    }

    #[test]
    fn the_login_target_is_a_path_this_portal_would_take_back(/* T-2290 */) {
        // The interrupted location is this request's own path and query, so it is the Portal's URI
        // and not a caller's — but the guard that reads it back on the login door is
        // `auth::oidc::safe_redirect`, which was tightened in T-2290. These two must not drift: a
        // target this builds has to be one that guard accepts, or a person is bounced to `/`
        // instead of back to what they were doing.
        for interrupted in [
            "/projects/helsinki/spaces",
            "/projects/helsinki/spaces?page=2&q=pm10%3E30",
            "/",
        ] {
            let built = login_redirect(interrupted);
            let target = built
                .strip_prefix("/login?redirect_to=")
                .expect("the login page with the location");
            let decoded: String =
                url::form_urlencoded::parse(format!("redirect_to={target}").as_bytes())
                    .find(|(key, _)| key == "redirect_to")
                    .map(|(_, value)| value.into_owned())
                    .expect("the parameter is there");
            assert_eq!(decoded, interrupted);
            assert_eq!(
                crate::auth::oidc::safe_redirect_for_tests(Some(decoded.clone())),
                decoded,
                "{interrupted} would be refused by the login door",
            );
        }
    }

    #[test]
    fn the_login_flow_and_assets_are_left_alone() {
        assert!(is_exempt("/api/v1/auth/login"));
        assert!(is_exempt("/api/v1/auth/callback"));
        assert!(is_exempt("/api/v1/auth/logout"));
        assert!(is_exempt("/api/v1/auth/backchannel-logout"));
        assert!(is_exempt("/assets/index-abc123.js"));
        assert!(is_exempt("/login"));
        assert!(!is_exempt("/api/v1/auth/me"));
        assert!(!is_exempt("/api/v1/projects/helsinki/spaces"));
        assert!(!is_exempt("/projects/helsinki/spaces"));
    }

    #[test]
    fn the_login_redirect_keeps_the_interrupted_location() {
        assert_eq!(
            login_redirect("/projects/helsinki/spaces?page=2"),
            "/login?redirect_to=%2Fprojects%2Fhelsinki%2Fspaces%3Fpage%3D2"
        );
    }

    #[test]
    fn only_an_html_navigation_is_redirected() {
        let nav = Request::builder()
            .header(header::ACCEPT, "text/html,application/xhtml+xml")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(is_navigation(&nav));
        let fetch = Request::builder()
            .header(header::ACCEPT, "application/json")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(!is_navigation(&fetch));
    }
}
