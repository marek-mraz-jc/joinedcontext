//! A signed-in mutation presents its session's own CSRF token (RTE-12, SEC-10): the cookie
//! helpers put `Token::bound` in the `__Host-jc_csrf` cookie, and a request echoes the cookie it
//! carries, as the UI does.

#![allow(dead_code)]

use axum::http::{header, request::Builder, HeaderMap};
use axum_extra::extract::cookie::{Key, PrivateCookieJar};
use joinedcontext_portal::auth::csrf::{Token, CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session;

/// The `__Host-jc_csrf` value in a `Cookie` header line.
pub fn csrf_of(cookies: &str) -> Option<&str> {
    cookies
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(CSRF_COOKIE)?.strip_prefix('='))
}

pub trait EchoCsrf {
    /// Sets `X-CSRF-Token` to the CSRF cookie the request's `Cookie` header already carries.
    fn echo_csrf(self) -> Self;
}

impl EchoCsrf for Builder {
    fn echo_csrf(self) -> Self {
        let token = self
            .headers_ref()
            .and_then(|headers| headers.get(header::COOKIE))
            .and_then(|value| value.to_str().ok())
            .and_then(csrf_of)
            .map(str::to_owned)
            .expect("echo_csrf: set a Cookie header holding the CSRF cookie first");
        self.header(CSRF_HEADER, token)
    }
}

/// `cookies`, a session's cookie line, with that session's CSRF cookie under `key` added.
pub fn with_bound_csrf(key: &Key, cookies: &str) -> String {
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, cookies.parse().expect("a cookie line"));
    let jar = PrivateCookieJar::from_headers(&headers, key.clone());
    let session = session::load(&jar).expect("with_bound_csrf: no session under this key");
    format!(
        "{cookies}; {CSRF_COOKIE}={}",
        Token::bound(key, &session).as_str()
    )
}

/// The CSRF token of the person an edge token names, as the App index or `/auth/me` hands it.
pub async fn edge_csrf(state: &joinedcontext_portal::state::AppState, edge_token: &str) -> String {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-access-token",
        edge_token.parse().expect("a header value"),
    );
    let session = joinedcontext_portal::auth::csrf::session_of(state, &headers)
        .await
        .expect("the edge token names a person");
    Token::bound(&state.config.cookie_key, &session)
        .as_str()
        .to_owned()
}
